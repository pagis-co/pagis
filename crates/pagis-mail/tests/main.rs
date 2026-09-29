//! One integration test binary for this crate. Each file under `tests/`
//! is a module here, so the crate links once instead of once per file.

mod address;
mod collector;
mod desk;
mod fakes;
mod greenmail;
mod live_mail;
mod live_migadu;
mod mail_events;
mod mail_tools;
mod manual;
mod migadu;
mod mime;
mod proof;
mod provider;
mod sender;
