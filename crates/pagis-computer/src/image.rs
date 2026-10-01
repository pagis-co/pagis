//! The Computer Image of the installation (ADR-0027). One preparation
//! of the pinned image serves every Workspace: the daemon starts it at
//! boot, and each wake that finds the image absent joins it. A
//! preparation pulls the pinned image when it is absent, checks its
//! version label, and then removes each other image of its repository
//! that no container uses.

use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

use crate::{ComputerError, ComputerRuntime, IMAGE, IMAGE_VERSION, ImageRemoval};

/// How far one preparation is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Preparation {
    /// The whole-pull percent.
    Pulling(u8),
    /// The pinned image is present, and the unused old images are gone.
    Ready,
    /// The preparation failed. The next wake or the next boot tries
    /// again.
    Failed(String),
}

/// The one preparation of the pinned Computer Image for the
/// installation. The Computer manager of every Workspace shares it, so
/// the installation pulls the image once.
pub struct ComputerImage {
    runtime: Arc<dyn ComputerRuntime>,
    /// The preparation that runs now. A join gets a copy of it, and a
    /// preparation that ends takes it out, so the next join starts a new
    /// one.
    running: Mutex<Option<watch::Receiver<Preparation>>>,
}

impl ComputerImage {
    pub fn new(runtime: Arc<dyn ComputerRuntime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            running: Mutex::new(None),
        })
    }

    /// Prepare the image at daemon boot, and return when the
    /// preparation ends. Where Docker does not answer, nothing happens,
    /// and the log says so at debug level only: a Local Installation
    /// without Docker is not an error.
    pub async fn prepare(self: &Arc<Self>) {
        if let Err(error) = self.runtime.image_version().await {
            tracing::debug!(%error, "Docker does not answer, so the Computer Image waits");
            return;
        }
        let mut preparation = self.join();
        // The preparation logs its own failure. An error here means that
        // the preparation stopped with no end, which it does only when
        // the daemon stops.
        let _ = preparation
            .wait_for(|now| !matches!(now, Preparation::Pulling(_)))
            .await;
    }

    /// Join the preparation that runs now, or start one.
    pub(crate) fn join(self: &Arc<Self>) -> watch::Receiver<Preparation> {
        let mut running = self.running.lock().expect("the computer image lock");
        // A closed channel is a preparation that stopped with no end.
        if let Some(preparation) = running.as_ref()
            && preparation.has_changed().is_ok()
        {
            return preparation.clone();
        }
        let (status, preparation) = watch::channel(Preparation::Pulling(0));
        *running = Some(preparation.clone());
        let image = Arc::clone(self);
        tokio::spawn(async move { image.run(status).await });
        preparation
    }

    async fn run(&self, status: watch::Sender<Preparation>) {
        let end = match self.make_present(&status).await {
            Ok(()) => {
                self.remove_unused().await;
                Preparation::Ready
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    image = IMAGE,
                    "the Computer Image is not ready; the next wake or boot tries again"
                );
                Preparation::Failed(error)
            }
        };
        // A join from here on starts a new preparation.
        self.running.lock().expect("the computer image lock").take();
        status.send_replace(end);
    }

    /// Pull the pinned image when it is absent, and check its version
    /// label. Each whole-pull percent goes to `status`.
    async fn make_present(&self, status: &watch::Sender<Preparation>) -> Result<(), String> {
        match self.runtime.image_version().await? {
            Some(version) if version == IMAGE_VERSION => return Ok(()),
            Some(found) => {
                return Err(ComputerError::VersionMismatch { found: Some(found) }.to_string());
            }
            None => {}
        }
        tracing::info!(image = IMAGE, "pulling the Computer Image");
        let (progress, mut percents) = mpsc::unbounded_channel();
        let forward = async {
            while let Some(percent) = percents.recv().await {
                status.send_if_modified(|now| {
                    let next = Preparation::Pulling(percent);
                    let changed = *now != next;
                    *now = next;
                    changed
                });
            }
        };
        let (pulled, ()) = tokio::join!(self.runtime.pull_image(progress), forward);
        pulled?;
        // The pulled image must carry the pinned version.
        let version = self.runtime.image_version().await?;
        if version.as_deref() != Some(IMAGE_VERSION) {
            return Err(ComputerError::VersionMismatch { found: version }.to_string());
        }
        tracing::info!(image = IMAGE, "the Computer Image is present");
        Ok(())
    }

    /// Remove each other image of the repository that no container
    /// uses. A removal never forces, so an image in use stays. A failure
    /// here does not fail the preparation: the pinned image is present.
    async fn remove_unused(&self) {
        let images = match self.runtime.other_images().await {
            Ok(images) => images,
            Err(error) => {
                tracing::warn!(%error, "the old Computer Images stay: the image list failed");
                return;
            }
        };
        for image in images {
            match self.runtime.remove_image(&image).await {
                Ok(ImageRemoval::Removed) => {
                    tracing::info!(%image, "removed an old Computer Image");
                }
                Ok(ImageRemoval::InUse) => {
                    tracing::debug!(%image, "an old Computer Image is in use and stays");
                }
                Err(error) => {
                    tracing::warn!(%error, %image, "an old Computer Image remove failed");
                }
            }
        }
    }
}
