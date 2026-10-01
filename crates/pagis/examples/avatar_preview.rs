//! A local preview with real storage and API routes, and fake external services.
//! Run with `cargo run -p pagis --example avatar_preview`.
use pagis_testkit::TestDaemon;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let daemon = TestDaemon::start().await;
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/settings/onboarding/complete",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await?
        .error_for_status()?;
    println!("PAGIS_DEV_API_URL={}", daemon.base_url);
    // The dev server proxies `/api`, so a sign-in link opened on its own
    // origin puts the session cookie where the page reads it.
    let sign_in = pagis::start_link(&daemon.booted.stores, "http://127.0.0.1:5188").await?;
    println!("Sign in within one minute: {sign_in}");
    println!("Then open http://127.0.0.1:5188/agents/{}", daemon.agent_id);
    println!("This is a temporary test workspace. Stop this process to remove it.");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
