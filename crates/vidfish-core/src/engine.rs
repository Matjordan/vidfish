use crate::config::AppConfig;
use tokio::{sync::watch, task::JoinHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineStatus {
    Starting,
    Running,
    Stopped,
}

/// Bounded control and status channels; no worker work runs on the UI thread.
pub struct Engine {
    stop: watch::Sender<bool>,
    status: watch::Receiver<EngineStatus>,
    task: JoinHandle<()>,
}

impl Engine {
    /// Must be called inside a Tokio runtime. Media workers will attach here.
    pub fn start(config: AppConfig) -> std::io::Result<Self> {
        config.validate()?;
        let (stop, mut stopping) = watch::channel(false);
        let (status_tx, status) = watch::channel(EngineStatus::Starting);
        let task = tokio::spawn(async move {
            tracing::info!(
                max_cameras = config.max_cameras,
                "camera manager started; network polling workers run independently"
            );
            status_tx.send_replace(EngineStatus::Running);
            // Closing the handle also requests shutdown; never detach a runaway worker.
            while !*stopping.borrow_and_update() {
                if stopping.changed().await.is_err() {
                    break;
                }
            }
            status_tx.send_replace(EngineStatus::Stopped);
            tracing::info!("camera manager stopped");
        });
        Ok(Self { stop, status, task })
    }
    pub fn status(&self) -> watch::Receiver<EngineStatus> {
        self.status.clone()
    }
    pub async fn shutdown(self) -> Result<(), tokio::task::JoinError> {
        self.stop.send_replace(true);
        self.task.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn lifecycle_runs_without_ui_and_shutdown_is_joined() {
        let engine = Engine::start(AppConfig::default()).unwrap();
        let mut status = engine.status();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while *status.borrow_and_update() != EngineStatus::Running {
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        engine.shutdown().await.unwrap();
        assert_eq!(*status.borrow(), EngineStatus::Stopped);
    }
    #[tokio::test]
    async fn dropping_control_handle_stops_worker() {
        let engine = Engine::start(AppConfig::default()).unwrap();
        let mut status = engine.status();
        drop(engine);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while *status.borrow_and_update() != EngineStatus::Stopped {
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }
}
