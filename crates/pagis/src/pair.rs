//! `pagis pair`: a Sign-In Link of the Public Origin for one more client,
//! made on the machine of the installation (ADR-0028).
//!
//! It gives a Headless Server its first Session, and it is the way back
//! in for a Person who has no Session left. The command writes the link
//! into the records of the installation directly, so it needs no running
//! daemon and no API: whoever can run it on the machine can already read
//! the records. The daemon spends the link when the client opens it, the
//! same as a link that a Person makes in Settings.

use std::path::Path;

use anyhow::Context;
use pagis_core::{CLIENT_LINK_LIFETIME_MS, Stores, UnixMillis, User, UserRole};

use crate::config::Config;

/// A link that `pagis pair` wrote, and the Person it signs in.
#[derive(Debug)]
pub struct Pairing {
    /// `<public origin>/sign-in#<secret>`.
    pub url: String,
    pub expires_at: UnixMillis,
    pub person: User,
}

/// Write a link for the Person with the address `email`, or for the
/// first Administrator of the installation, into the installation at
/// `home`. The link starts at the configured Public Origin and is good
/// for five minutes and one use.
pub async fn pair(home: &Path, email: Option<&str>, now: UnixMillis) -> anyhow::Result<Pairing> {
    let config_file = home.join("config.toml");
    if !config_file.exists() {
        anyhow::bail!(
            "no Pagis installation is at {}; start pagis first, or set PAGIS_HOME",
            home.display()
        );
    }
    let config = Config::load_or_init(&config_file)?;
    let stores = open_stores(home, &config).await?;
    let person = person(&stores, email).await?;
    let link = pagis_server::mint_public_origin_link(
        stores.sign_in_links.as_ref(),
        &person.id,
        &config.public_origin(config.port),
        CLIENT_LINK_LIFETIME_MS,
        now,
    )
    .await
    .context("write the sign-in link")?;
    Ok(Pairing {
        url: link.url,
        expires_at: link.expires_at,
        person,
    })
}

/// The records the daemon of this installation keeps, with no migration:
/// the daemon owns the schema, and it may run now.
async fn open_stores(home: &Path, config: &Config) -> anyhow::Result<Stores> {
    match config.database.url() {
        Some(url) => {
            let pool = pagis_storage_postgres::connect(url)
                .await
                .context("open the Postgres database of the installation")?;
            Ok(pagis_storage_postgres::stores(pool))
        }
        None => {
            let database = home.join("pagis.db");
            if !database.exists() {
                anyhow::bail!("{} holds no records yet; start pagis first", home.display());
            }
            let pool = pagis_storage_sqlite::connect(&database)
                .await
                .with_context(|| format!("open {}", database.display()))?;
            Ok(pagis_storage_sqlite::stores(pool))
        }
    }
}

/// The Person the link signs in: the one with this address, or the first
/// Administrator. A disabled Person signs in to nothing, so the command
/// refuses one.
async fn person(stores: &Stores, email: Option<&str>) -> anyhow::Result<User> {
    let person = match email {
        Some(email) => stores.users.find_by_email(email).await?.ok_or_else(|| {
            anyhow::anyhow!("no Person of this installation has the address {email}")
        })?,
        None => {
            let org = stores
                .orgs
                .list()
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    anyhow::anyhow!("the installation has no Org yet; start pagis first")
                })?;
            stores
                .users
                .list_by_org(&org.id)
                .await?
                .into_iter()
                .find(|user| user.role == UserRole::Administrator && !user.is_disabled())
                .ok_or_else(|| anyhow::anyhow!("the installation has no Administrator"))?
        }
    };
    if person.is_disabled() {
        anyhow::bail!(
            "the account of {} is disabled; an Administrator enables it first",
            person.email.as_deref().unwrap_or("this Person")
        );
    }
    Ok(person)
}
