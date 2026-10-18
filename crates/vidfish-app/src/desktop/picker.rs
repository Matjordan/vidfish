//! Native dialogs run on a worker; acquisition and egui keep progressing.
use super::*;
#[derive(Clone, Copy)]
pub(super) enum Target {
    Output,
    Export,
}
type Selection = (Target, Result<Option<PathBuf>, String>);
#[derive(Default)]
pub(super) struct Picker {
    job: Option<tokio::sync::oneshot::Receiver<Selection>>,
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
        let (tx, rx) = tokio::sync::oneshot::channel();
        runtime.spawn_blocking(move || {
            while !current.is_dir() && current.pop() {}
            let mut dialog = rfd::FileDialog::new().set_title("Choose destination folder");
            if current.is_dir() {
                dialog = dialog.set_directory(current);
            }
            let selection=dialog.pick_folder().map(|path| {
                tempfile::NamedTempFile::new_in(&path).map_err(|_|"The selected folder is not writable. Choose a writable output/export folder.".to_string())?;
                Ok(path)
            }).transpose();
            let _ = tx.send((target, selection));
        });
        self.job = Some(rx);
    }
    pub fn poll(&mut self) -> Option<Result<(Target, PathBuf), String>> {
        let result = self.job.as_mut()?.try_recv();
        match result {
            Ok((target, path)) => {
                self.job = None;
                match path {
                    Ok(path) => path.map(|p| Ok((target, p))),
                    Err(error) => Some(Err(error)),
                }
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.job = None;
                Some(Err(
                    "The native folder dialog could not finish; retry Browse or paste the path."
                        .into(),
                ))
            }
            Err(_) => None,
        }
    }
}
