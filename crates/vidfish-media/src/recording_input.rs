//! A replaceable RTSP reader feeding a persistent stream-copy writer.
use crate::{Codec, MediaError, Result, Source, encoded_source};
use gst::prelude::*;
use gstreamer as gst;
use std::sync::{Arc, Mutex};

const MAX_BUFFERS: u64 = 30;
const MAX_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) type Timeline = Arc<Mutex<u64>>;

pub(crate) fn writer_source() -> gstreamer_app::AppSrc {
    gstreamer_app::AppSrc::builder()
        .name("recording-input")
        .format(gst::Format::Time)
        .is_live(true)
        .block(false)
        .max_buffers(MAX_BUFFERS)
        .max_bytes(MAX_BYTES)
        .build()
}

pub(crate) fn start(
    source: &Source,
    codec: Codec,
    writer: &gst::Pipeline,
    destination: &gstreamer_app::AppSrc,
    timeline: Timeline,
) -> Result<gst::Pipeline> {
    let input = gst::Pipeline::new();
    let parser = encoded_source(&input, source, codec)?;
    parser.set_property("config-interval", -1i32);
    if let Some(depay) = input.by_name("depay") {
        for property in ["request-keyframe", "wait-for-keyframe"] {
            if depay.find_property(property).is_some() {
                depay.set_property(property, true);
            }
        }
    }
    let sink = gstreamer_app::AppSink::builder()
        .name("encoded-input")
        .caps(
            &gst::Caps::builder(match codec {
                Codec::H264 => "video/x-h264",
                Codec::H265 => "video/x-h265",
            })
            .field("stream-format", "byte-stream")
            .field("alignment", "au")
            .build(),
        )
        .sync(false)
        .enable_last_sample(false)
        .max_buffers(1)
        .build();
    input
        .add(&sink)
        .map_err(|_| MediaError("cannot add recording input bridge".into()))?;
    parser
        .link(&sink)
        .map_err(|_| MediaError("cannot link recording input bridge".into()))?;
    let writer = writer.clone();
    let destination = destination.clone();
    let mut origin = None;
    sink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let caps = sample.caps_owned().ok_or(gst::FlowError::Error)?;
                let mut buffer = sample.buffer_owned().ok_or(gst::FlowError::Error)?;
                // A new connection cannot safely append dependent frames from a damaged GOP.
                if origin.is_none() && buffer.flags().contains(gst::BufferFlags::DELTA_UNIT) {
                    return Ok(gst::FlowSuccess::Ok);
                }
                // Bound retained encoded data without silently dropping recording frames.
                // A full writer queue produces an input failure, then the normal loss policy.
                if buffer.size() as u64 > MAX_BYTES
                    || destination.current_level_buffers() >= MAX_BUFFERS
                    || destination
                        .current_level_bytes()
                        .saturating_add(buffer.size() as u64)
                        > MAX_BYTES
                {
                    return Err(gst::FlowError::Error);
                }
                let timestamp = buffer.dts_or_pts().ok_or(gst::FlowError::Error)?.nseconds();
                let (input_base, output_base) = *origin.get_or_insert_with(|| {
                    destination.set_caps(Some(&caps));
                    let running = writer.current_running_time().map_or(0, |t| t.nseconds());
                    (timestamp, running.max(*timeline.lock().unwrap()))
                });
                // Reconnecting cameras may reset RTP timestamps. Preserve PTS/DTS offsets
                // (including reordered frames) in the writer's uninterrupted running-time domain.
                let translate = |t: gst::ClockTime| {
                    gst::ClockTime::from_nseconds(
                        (i128::from(output_base) + i128::from(t.nseconds())
                            - i128::from(input_base))
                        .clamp(0, i128::from(u64::MAX)) as u64,
                    )
                };
                let writable = buffer.make_mut();
                writable.set_pts(writable.pts().map(translate));
                writable.set_dts(writable.dts().map(translate));
                let end = writable
                    .pts()
                    .or(writable.dts())
                    .map_or(output_base, |t| t.nseconds())
                    .saturating_add(writable.duration().map_or(1, |t| t.nseconds().max(1)));
                {
                    let mut next = timeline.lock().unwrap();
                    *next = (*next).max(end);
                }
                destination.push_buffer(buffer)
            })
            .build(),
    );
    if input.set_state(gst::State::Playing).is_err() {
        let _ = input.set_state(gst::State::Null);
        return Err(MediaError("cannot start recording input connection".into()));
    }
    Ok(input)
}
