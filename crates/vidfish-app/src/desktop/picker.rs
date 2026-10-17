//! Native dialogs run on a worker; acquisition and egui keep progressing.
use super::*;
#[derive(Clone, Copy)]
pub(super) enum Target {
    Output,
    Export,
}
#[derive(Default)]
pub(super) struct Picker {
    job: Option<tokio::sync::oneshot::Receiver<(Target, Option<PathBuf>)>>,
}
impl Picker {
    pub fn folder(
        &mut self,
        runtime: &tokio::runtime::Handle,
        target: Target,
        mut current: PathBuf,
    ) {
        if self.job.is_some() {
            return;
        }
        while !current.is_dir() && current.pop() {}
        let (tx, rx) = tokio::sync::oneshot::channel();
        runtime.spawn_blocking(move || {
            let mut dialog = rfd::FileDialog::new().set_title("Choose destination folder");
            if current.is_dir() {
                dialog = dialog.set_directory(current);
            }
            let _ = tx.send((target, dialog.pick_folder()));
        });
        self.job = Some(rx);
    }
    pub fn poll(&mut self) -> Option<(Target, PathBuf)> {
        let result = self.job.as_mut()?.try_recv();
        match result {
            Ok((target, path)) => {
                self.job = None;
                path.map(|p| (target, p))
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.job = None;
                None
            }
            Err(_) => None,
        }
    }
}
