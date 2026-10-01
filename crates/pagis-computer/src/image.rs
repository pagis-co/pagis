//! The Computer Image of the installation (ADR-0027). One preparation
//! of the pinned image serves every Workspace: the daemon starts it at
//! boot, and each wake that finds the image absent joins it. A
//! preparation pulls the pinned image when it is absent, checks its
//! version label, and then removes each other image of its repository
//! that no container uses and that is not newer than the pin.
//!
//! Before a restart to an Update, the Client App asks the daemon to pull
//! the Computer Image of the next release ([`ComputerImage::pull`]). That
//! image is newer than the pin, so it stays until the daemon of the next
//! release runs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

use crate::{
    ComputerError, ComputerRuntime, IMAGE, IMAGE_VERSION, ImageRemoval, OtherImage,
    image_repository,
};

/// Why a pull of a named Computer Image did not give the image.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImagePullError {
    /// The name is not an image of the Computer Image repository by its
    /// digest.
    #[error("{0}")]
    Refused(String),
    /// Docker does not answer.
    #[error("Docker does not answer: {0}")]
    NoDocker(String),
    /// The pull started and failed.
    #[error("the pull of the Computer Image failed: {0}")]
    Failed(String),
}

/// The end of one pull of a named image: `None` while it runs.
type PullEnd = Option<Result<(), String>>;

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
    /// The pulls of named images that run now, by name. They work as
    /// `running` does.
    pulls: Mutex<HashMap<String, watch::Receiver<PullEnd>>>,
}

impl ComputerImage {
    pub fn new(runtime: Arc<dyn ComputerRuntime>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            running: Mutex::new(None),
            pulls: Mutex::new(HashMap::new()),
        })
    }

    /// Pull `image`, an image of the Computer Image repository by its
    /// digest, and return when the pull ends. The Client App asks for the
    /// image of the next release before it restarts to an Update. A
    /// second request for the same image joins the pull that runs. The
    /// pull runs in a task of its own, so a request that goes away does
    /// not stop it.
    pub async fn pull(self: &Arc<Self>, image: &str) -> Result<(), ImagePullError> {
        check_reference(image)?;
        if let Err(error) = self.runtime.image_version().await {
            return Err(ImagePullError::NoDocker(error));
        }
        let mut pull = self.join_pull(image);
        let end = pull
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|end| end.clone());
        match end {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => Err(ImagePullError::Failed(error)),
            // The task stopped with no end, which it does only when the
            // daemon stops.
            None => Err(ImagePullError::Failed(
                "the pull stopped before it ended".to_string(),
            )),
        }
    }

    /// Join the pull of `image` that runs now, or start one.
    fn join_pull(self: &Arc<Self>, image: &str) -> watch::Receiver<PullEnd> {
        let mut pulls = self.pulls.lock().expect("the computer image lock");
        // A closed channel is a pull that stopped with no end.
        if let Some(pull) = pulls.get(image)
            && pull.has_changed().is_ok()
        {
            return pull.clone();
        }
        let (end, pull) = watch::channel(None);
        pulls.insert(image.to_string(), pull.clone());
        let this = Arc::clone(self);
        let image = image.to_string();
        tokio::spawn(async move {
            tracing::info!(%image, "pulling a Computer Image that the Client App asked for");
            // Nobody reads the percents of this pull.
            let (progress, _percents) = mpsc::unbounded_channel();
            let pulled = this.runtime.pull_image(&image, progress).await;
            match &pulled {
                Ok(()) => tracing::info!(%image, "the Computer Image is present"),
                Err(error) => tracing::warn!(%error, %image, "the Computer Image pull failed"),
            }
            // A request from here on starts a new pull.
            this.pulls
                .lock()
                .expect("the computer image lock")
                .remove(&image);
            end.send_replace(Some(pulled));
        });
        pull
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
        let (pulled, ()) = tokio::join!(self.runtime.pull_image(IMAGE, progress), forward);
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
    /// uses. A removal never forces, so an image in use stays. An image
    /// whose version is newer than the pin stays too: the Client App had
    /// it pulled for the next release. A failure here does not fail the
    /// preparation: the pinned image is present.
    async fn remove_unused(&self) {
        let images = match self.runtime.other_images().await {
            Ok(images) => images,
            Err(error) => {
                tracing::warn!(%error, "the old Computer Images stay: the image list failed");
                return;
            }
        };
        for OtherImage { id: image, version } in images {
            if is_newer_than_pin(version.as_deref()) {
                tracing::debug!(%image, ?version, "a newer Computer Image stays for the next release");
                continue;
            }
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

/// Whether `version`, a version label, is a SemVer version newer than
/// the pinned one. An image with no label, or with a label that is not
/// SemVer, is not.
fn is_newer_than_pin(version: Option<&str>) -> bool {
    let (Some(version), Ok(pinned)) = (version, semver::Version::parse(IMAGE_VERSION)) else {
        return false;
    };
    semver::Version::parse(version).is_ok_and(|version| version > pinned)
}

/// Refuse a name that is not `<repository>@sha256:<64 lowercase
/// hexadecimal characters>`, where the repository is the repository of
/// the pinned image. A digest names exactly one image, and a tag can
/// move.
fn check_reference(image: &str) -> Result<(), ImagePullError> {
    let repository = image_repository(IMAGE);
    if image_repository(image) != repository {
        return Err(ImagePullError::Refused(format!(
            "{image} is not an image of the Computer Image repository {repository}"
        )));
    }
    match image
        .strip_prefix(repository)
        .and_then(|rest| rest.strip_prefix("@sha256:"))
    {
        Some(digest)
            if digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            Ok(())
        }
        _ => Err(ImagePullError::Refused(format!(
            "{image} names no digest: the daemon pulls a Computer Image only as \
             {repository}@sha256:<64 lowercase hexadecimal characters>"
        ))),
    }
}
