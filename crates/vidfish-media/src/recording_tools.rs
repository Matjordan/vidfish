//! Local-file diagnostics and non-destructive stream-copy recovery.
use crate::{Codec, MediaError, Result, bus_error, element, initialize, inspect_recording};
use gst::prelude::*;
use gstreamer as gst;
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// Decode and seek with software decoders, independent of the live viewer.
/// Returns a packed RGB frame so callers can independently inspect actual pixels.
pub fn software_frame(path: &Path, seconds: f64) -> Result<(u32, u32, Vec<u8>)> {
    initialize()?;
    let pipeline = gst::Pipeline::new();
    let source = element("filesrc", "source")?;
    source.set_property(
        "location",
        path.to_str()
            .ok_or_else(|| MediaError("non-UTF8 media path".into()))?,
    );
    let decoder = element("decodebin", "software-decoder")?;
    decoder.set_property("force-sw-decoders", true);
    let convert = element("videoconvert", "convert")?;
    let sink = gstreamer_app::AppSink::builder()
        .sync(false)
        .max_buffers(1)
        .drop(true)
        .caps(
            &gst::Caps::builder("video/x-raw")
                .field("format", "RGB")
                .build(),
        )
        .build();
    pipeline
        .add_many([&source, &decoder, &convert, sink.upcast_ref()])
        .map_err(|_| MediaError("cannot assemble software inspection".into()))?;
    source
        .link(&decoder)
        .map_err(|_| MediaError("cannot link software decoder".into()))?;
    convert
        .link(&sink)
        .map_err(|_| MediaError("cannot link inspected frame".into()))?;
    let input = convert.static_pad("sink").unwrap();
    decoder.connect_pad_added(move |_, pad| {
        if !input.is_linked()
            && pad.current_caps().is_some_and(|caps| {
                caps.structure(0)
                    .is_some_and(|s| s.name().starts_with("video/"))
            })
        {
            let _ = pad.link(&input);
        }
    });
    let result = (|| {
        pipeline
            .set_state(gst::State::Paused)
            .map_err(|_| MediaError("software decoder startup failed".into()))?;
        pipeline
            .state(gst::ClockTime::from_seconds(10))
            .0
            .map_err(|_| MediaError("software decoder preroll failed".into()))?;
        if seconds > 0.0 {
            pipeline
                .seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                    gst::ClockTime::from_nseconds((seconds * 1e9) as u64),
                )
                .map_err(|_| MediaError("recording seek failed".into()))?;
        }
        let sample = sink
            .try_pull_preroll(gst::ClockTime::from_seconds(10))
            .ok_or_else(|| MediaError("no software-decoded frame at requested offset".into()))?;
        let info = gstreamer_video::VideoInfo::from_caps(
            sample
                .caps()
                .ok_or_else(|| MediaError("frame caps missing".into()))?,
        )
        .map_err(|_| MediaError("invalid inspected frame".into()))?;
        let buffer = sample
            .buffer()
            .ok_or_else(|| MediaError("frame missing".into()))?;
        let map = buffer
            .map_readable()
            .map_err(|_| MediaError("cannot read inspected frame".into()))?;
        let mut rgb = Vec::with_capacity(info.width() as usize * info.height() as usize * 3);
        for row in 0..info.height() as usize {
            let start = info.offset()[0] + row * info.stride()[0] as usize;
            rgb.extend_from_slice(&map[start..start + info.width() as usize * 3]);
        }
        Ok((info.width(), info.height(), rgb))
    })();
    let _ = pipeline.set_state(gst::State::Null);
    result
}

/// Rebuild indexing into a new file, without re-encoding or modifying the source.
/// A caller must hold the recorder ownership lock before recovering active-run files.
pub fn remux(source_path: &Path, output: &Path, codec: Codec, container: &str) -> Result<()> {
    initialize()?;
    if !matches!(container, "mkv" | "mp4") {
        return Err(MediaError("unsupported remux container".into()));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|_| MediaError("recovery destination already exists or is not writable".into()))?;
    let pipeline = gst::Pipeline::new();
    let result = (|| {
        let source = element("filesrc", "source")?;
        source.set_property(
            "location",
            source_path
                .to_str()
                .ok_or_else(|| MediaError("non-UTF8 media path".into()))?,
        );
        let demux = element(
            if source_path.extension().is_some_and(|s| s == "mp4") {
                "qtdemux"
            } else {
                "matroskademux"
            },
            "demux",
        )?;
        let parser = element(
            match codec {
                Codec::H264 => "h264parse",
                Codec::H265 => "h265parse",
            },
            "parser",
        )?;
        parser.set_property("config-interval", -1i32);
        let mux = element(
            if container == "mp4" {
                "mp4mux"
            } else {
                "matroskamux"
            },
            "mux",
        )?;
        let sink = element("filesink", "output")?;
        sink.set_property(
            "location",
            output
                .to_str()
                .ok_or_else(|| MediaError("non-UTF8 recovery path".into()))?,
        );
        pipeline
            .add_many([&source, &demux, &parser, &mux, &sink])
            .map_err(|_| MediaError("cannot assemble recovery".into()))?;
        source
            .link(&demux)
            .map_err(|_| MediaError("cannot link recovery source".into()))?;
        gst::Element::link_many([&parser, &mux, &sink])
            .map_err(|_| MediaError("cannot link recovery output".into()))?;
        let input = parser.static_pad("sink").unwrap();
        let media = match codec {
            Codec::H264 => "video/x-h264",
            Codec::H265 => "video/x-h265",
        };
        demux.connect_pad_added(move |_, pad| {
            if !input.is_linked()
                && pad
                    .current_caps()
                    .is_some_and(|caps| caps.structure(0).is_some_and(|s| s.name() == media))
            {
                let _ = pad.link(&input);
            }
        });
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| MediaError("cannot start recovery remux".into()))?;
        let bus = pipeline.bus().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() >= deadline {
                return Err(MediaError(
                    "recovery remux timed out; original retained".into(),
                ));
            }
            if let Some(message) = bus.timed_pop_filtered(
                gst::ClockTime::from_mseconds(100),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            ) {
                if message.type_() == gst::MessageType::Error {
                    return Err(bus_error(&message));
                }
                break;
            }
        }
        pipeline
            .set_state(gst::State::Null)
            .map_err(|_| MediaError("cannot close recovered file".into()))?;
        let info = inspect_recording(output)?;
        if !info.seekable || info.duration_seconds <= 0.0 {
            return Err(MediaError(
                "recovered file failed duration/seek validation".into(),
            ));
        }
        software_frame(output, info.duration_seconds / 2.0)?;
        std::fs::File::open(output)
            .and_then(|file| file.sync_all())
            .map_err(|_| MediaError("cannot persist recovered file".into()))?;
        Ok(())
    })();
    let _ = pipeline.set_state(gst::State::Null);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}
