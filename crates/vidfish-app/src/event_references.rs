//! Correlate the recorder's sampled GStreamer clock with actual split boundaries.
//! UTC extrapolation is an estimate; never label it frame-exact synchronization.
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Reference {
    pub run_id: Option<String>,
    pub journal: Option<PathBuf>,
    pub running_time_ns: Option<u64>,
    pub segment: Option<PathBuf>,
    pub offset_ns: Option<u64>,
    pub state: String,
    pub timing: String,
    pub sample_age_ms: Option<u64>,
}
impl Reference {
    pub fn at(status: Option<&crate::acquisition::Status>, action_ms: u64) -> Self {
        let Some(s) = status else {
            return Self {
                state: "No recording status / disconnected".into(),
                ..Self::default()
            };
        };
        let mut r = Self {
            run_id: Some(s.run_id.clone()),
            state: format!("{:?}", s.state),
            ..Self::default()
        };
        if s.state != crate::acquisition::State::Recording {
            return r;
        }
        let Some(parent) = s.output.as_ref().and_then(|p| p.parent()) else {
            r.state = "Recording has no segment yet".into();
            return r;
        };
        r.journal = Some(parent.join("segments.jsonl"));
        let age = action_ms.checked_sub(s.media_observed_unix_ms);
        r.sample_age_ms = age;
        if let (Some(t), Some(age)) = (s.media_running_time_ns, age.filter(|a| *a <= 3000)) {
            r.running_time_ns = t.checked_add(age.saturating_mul(1_000_000));
            r.timing="Recorder GStreamer clock extrapolated with UTC; approximate, subject to clock changes/network buffering".into();
            r.resolve();
        } else {
            r.state = "Gap: stale/unavailable recorder clock".into();
        }
        r
    }
    pub fn resolve(&mut self) {
        let (Some(path), Some(at)) = (&self.journal, self.running_time_ns) else {
            return;
        };
        let Ok(file) = std::fs::File::open(path) else {
            self.state = "Segment index missing".into();
            return;
        };
        let rows: Vec<vidfish_media::segmented::FragmentEvent> = BufReader::new(file)
            .lines()
            .take(100_000)
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect();
        let chosen = rows
            .iter()
            .filter(|r| r.event == "opened" && r.running_time_ns <= at)
            .max_by_key(|r| r.running_time_ns);
        let Some(start) = chosen else {
            self.state = "Pending: no observed segment boundary at marker".into();
            return;
        };
        let close = rows
            .iter()
            .find(|r| r.event == "closed" && r.location == start.location);
        // Half-open intervals put an event exactly on a split into the next fragment.
        if close.is_some_and(|end| at >= end.running_time_ns) {
            self.segment = None;
            self.offset_ns = None;
            self.state = "Gap or pending next segment boundary".into();
            return;
        }
        self.segment = Some(start.location.clone());
        self.offset_ns = Some(at - start.running_time_ns);
        self.state = if !start.location.exists() {
            "Referenced media missing"
        } else if close.is_some() {
            "Closed segment; approximate clock offset"
        } else {
            "Pending segment close; approximate clock offset"
        }
        .into();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_boundary_uses_actual_clock_and_reconciles_after_close() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mkv");
        let b = dir.path().join("b.mkv");
        std::fs::write(&a, []).unwrap();
        std::fs::write(&b, []).unwrap();
        let journal = dir.path().join("segments.jsonl");
        let row = |event: &str, location: &PathBuf, t| {
            serde_json::to_string(&vidfish_media::segmented::FragmentEvent {
                event: event.into(),
                location: location.clone(),
                running_time_ns: t,
                observed_unix_ms: 0,
            })
            .unwrap()
                + "\n"
        };
        std::fs::write(&journal, row("opened", &a, 500)).unwrap();
        let mut r = Reference {
            journal: Some(journal.clone()),
            running_time_ns: Some(1000),
            ..Reference::default()
        };
        r.resolve();
        assert!(r.state.starts_with("Pending"));
        std::fs::write(
            &journal,
            row("opened", &a, 500) + &row("closed", &a, 1000) + &row("opened", &b, 1000),
        )
        .unwrap();
        r.resolve();
        assert_eq!(r.segment, Some(b.clone()));
        assert_eq!(r.offset_ns, Some(0));
        std::fs::write(
            &journal,
            row("opened", &a, 500)
                + &row("closed", &a, 1000)
                + &row("opened", &b, 1000)
                + &row("closed", &b, 2000),
        )
        .unwrap();
        r.resolve();
        assert!(r.state.starts_with("Closed"));
        std::fs::remove_file(&b).unwrap();
        r.resolve();
        assert_eq!(r.state, "Referenced media missing");
    }
}
