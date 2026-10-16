use pagis_core::SpamAction;
use pagis_mail::{HostSpamVerdicts, MessageId, SpamVerdictSource, read_mime};

fn verdict(headers: &str, folder: &str) -> pagis_core::SpamVerdict {
    let message = read_mime(
        format!("From: sender@example.com\r\n{headers}\r\n\r\nBody").as_bytes(),
        MessageId::new(format!("{folder}:1")),
    )
    .unwrap();
    pagis_mail::parse_spam_verdict(&message.summary.spam_headers, folder == "Junk")
}

#[test]
fn folded_spamassassin_status_has_score_and_tests() {
    let v = verdict(
        "X-Spam-Status: Yes, score=7.2 required=5.0 tests=GTUBE,\r\n\tURIBL_BLOCKED autolearn=no\r\nX-Spam-Flag: YES",
        "INBOX",
    );
    assert_eq!(v.score, 7200);
    assert_eq!(v.action, SpamAction::AddHeader);
    assert_eq!(v.reasons, ["GTUBE", "URIBL_BLOCKED"]);
}

#[test]
fn rspamd_actions_and_negative_scores() {
    for (name, action) in [
        ("no action", SpamAction::NoAction),
        ("add header", SpamAction::AddHeader),
        ("rewrite subject", SpamAction::RewriteSubject),
        ("soft reject", SpamAction::SoftReject),
        ("reject", SpamAction::Reject),
    ] {
        let v = verdict(
            &format!("X-Rspamd-Score: -1.25 / 15.00\r\nX-Rspamd-Action: {name}"),
            "INBOX",
        );
        assert_eq!(v.score, -1250);
        assert_eq!(v.action, action);
    }
}

#[test]
fn junk_folder_overrides_headers_and_absent_headers() {
    for headers in ["", "X-Spam-Score: 0\r\nX-Rspamd-Action: no action"] {
        assert_eq!(verdict(headers, "Junk").action, SpamAction::Junk);
    }
    assert_eq!(verdict("", "INBOX").action, SpamAction::NoAction);
}

#[test]
fn score_flag_level_and_invalid_numbers() {
    assert_eq!(verdict("X-Spam-Score: 4.5", "INBOX").score, 4500);
    assert_eq!(verdict("X-Spam-Level: ****", "INBOX").score, 4000);
    assert_eq!(
        verdict("X-Spam-Flag: YES", "INBOX").action,
        SpamAction::AddHeader
    );
    for number in ["NaN", "inf", "garbage"] {
        assert_eq!(
            verdict(&format!("X-Spam-Score: {number}"), "INBOX").score,
            0
        );
    }
}

pub fn context() -> pagis_mail::MailReadContext {
    let stores = pagis_core::fake::FakeDatabase::default().stores();
    pagis_mail::MailReadContext {
        intake: std::sync::Arc::new(pagis_mail::MailIntake {
            opinions: std::sync::Arc::new(pagis_mail::fake::FakeMailOpinions::default()),
            workspaces: stores.workspaces.clone(),
            agents: stores.agents.clone(),
            source: std::sync::Arc::new(HostSpamVerdicts),
            records: stores.mail_assessments,
            trust: std::sync::Arc::new(pagis_mail::ListedSenders::new(
                stores.trust_list,
                stores.connections,
            )),
        }),
        workspace: pagis_core::WorkspaceId::generate(),
        address: "person@example.com".into(),
        agent: None,
        authserv_ids: Vec::new(),
    }
}

#[tokio::test]
async fn a_failed_source_stores_nothing_and_retries() {
    let mut context = context();
    let source = std::sync::Arc::new(pagis_mail::fake::FakeSpamVerdicts::default());
    let records = context.intake.records.clone();
    context.intake = std::sync::Arc::new(pagis_mail::MailIntake {
        opinions: std::sync::Arc::new(pagis_mail::fake::FakeMailOpinions::default()),
        workspaces: context.intake.workspaces.clone(),
        agents: context.intake.agents.clone(),
        source: source.clone(),
        records: records.clone(),
        trust: context.intake.trust.clone(),
    });
    let mut message =
        read_mime(b"From: sender@test\r\n\r\nbody", MessageId::new("INBOX:1")).unwrap();
    *source.0.lock().unwrap() = Err(pagis_mail::TransportError(
        pagis_mail::TransportErrorCode::Unreachable,
    ));
    assert!(context.assess(&mut message.summary, false).await.is_err());
    assert!(message.summary.assessment.is_none());
    assert!(
        records
            .get(&context.workspace, &context.address, "INBOX:1")
            .await
            .unwrap()
            .is_none()
    );
    *source.0.lock().unwrap() = Ok(pagis_core::SpamVerdict {
        opinion: None,
        score: 8100,
        action: SpamAction::Reject,
        reasons: vec!["test verdict".into()],
        source: "fake".into(),
    });
    context.assess(&mut message.summary, false).await.unwrap();
    let saved = records
        .get(&context.workspace, &context.address, "INBOX:1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.verdict.score, 8100);
    assert_eq!(saved.trust_tier, pagis_core::TrustTier::Unknown);
}

#[tokio::test]
async fn verdict_precedes_trust_and_storage_precedes_a_read() {
    use async_trait::async_trait;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Source(Arc<AtomicUsize>);
    struct Trust(Arc<AtomicUsize>);
    #[async_trait]
    impl SpamVerdictSource for Source {
        async fn verdict(
            &self,
            _: &pagis_mail::MessageSummary,
            _: bool,
        ) -> Result<pagis_core::SpamVerdict, pagis_mail::TransportError> {
            assert_eq!(self.0.fetch_add(1, Ordering::SeqCst), 0);
            Ok(pagis_core::SpamVerdict {
                opinion: None,
                score: 1200,
                action: SpamAction::NoAction,
                reasons: vec![],
                source: "fake".into(),
            })
        }
    }
    #[async_trait]
    impl pagis_mail::SenderTrust for Trust {
        async fn tier(
            &self,
            _: &pagis_core::WorkspaceId,
            _: &pagis_core::AgentId,
            _: &pagis_mail::SenderEvidence,
        ) -> Result<pagis_core::TrustTier, pagis_core::StoreError> {
            assert_eq!(self.0.fetch_add(1, Ordering::SeqCst), 1);
            Ok(pagis_core::TrustTier::Trusted)
        }
    }
    let order = Arc::new(AtomicUsize::new(0));
    let mut context = context();
    context.agent = Some(pagis_core::AgentId::generate());
    context.intake = Arc::new(pagis_mail::MailIntake {
        opinions: std::sync::Arc::new(pagis_mail::fake::FakeMailOpinions::default()),
        workspaces: context.intake.workspaces.clone(),
        agents: context.intake.agents.clone(),
        source: Arc::new(Source(order.clone())),
        records: context.intake.records.clone(),
        trust: Arc::new(Trust(order.clone())),
    });
    let mut message =
        read_mime(b"From: sender@test\r\n\r\nbody", MessageId::new("INBOX:1")).unwrap();
    context.assess(&mut message.summary, false).await.unwrap();
    assert_eq!(order.load(Ordering::SeqCst), 2);
    let stored = context
        .intake
        .records
        .get(&context.workspace, &context.address, "INBOX:1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(message.summary.assessment, Some(stored.clone()));
    assert_eq!(stored.trust_tier, pagis_core::TrustTier::Trusted);
}

#[test]
fn rspamd_result_has_a_score_and_weighted_symbols() {
    let headers = "X-Spamd-Result: default: True [7.20 / 15.00];\r\n\tBAYES_SPAM(5.10)[99.99%];\r\n\tR_SPF_FAIL(1.00)[-all]; MIME_GOOD(-0.10)[text/plain]\r\nX-Rspamd-Action: add header";
    let v = verdict(headers, "INBOX");
    assert_eq!(v.score, 7200);
    assert_eq!(v.action, SpamAction::AddHeader);
    assert_eq!(
        v.reasons,
        ["BAYES_SPAM 5.10", "R_SPF_FAIL 1.00", "MIME_GOOD -0.10"]
    );
    assert_eq!(
        verdict(&format!("X-Rspamd-Score: 8.3\r\n{headers}"), "INBOX").score,
        8300
    );
}

#[tokio::test]
async fn the_host_source_uses_the_pure_parser() {
    let message = read_mime(b"From: sender@test\r\nX-Spamd-Result: default: True [7.20 / 15.00]; BAYES_SPAM(5.10)[99.99%]\r\n\r\nbody", MessageId::new("INBOX:1")).unwrap();
    assert_eq!(
        HostSpamVerdicts
            .verdict(&message.summary, false)
            .await
            .unwrap(),
        pagis_mail::parse_spam_verdict(&message.summary.spam_headers, false)
    );
}

#[test]
fn rspamd_symbol_options_and_invalid_weights_are_not_reasons() {
    let v = verdict(
        "X-Spamd-Result: default: False [-1.25 / 15.00]; MIME_GOOD(-1.25)[text/plain; OTHER(9.00)]; INVALID(NaN)[]",
        "INBOX",
    );
    assert_eq!(v.score, -1250);
    assert_eq!(v.reasons, ["MIME_GOOD -1.25"]);
}

#[tokio::test]
async fn only_uncertain_mail_gets_one_opinion_and_stored_mail_is_not_judged_again() {
    use std::sync::{Arc, atomic::Ordering};
    for (score, junk, action, calls) in [
        (3999, false, "no action", 0),
        (4000, false, "no action", 1),
        (14900, false, "add header", 1),
        (15000, false, "no action", 0),
        (8000, true, "no action", 0),
        (8000, false, "reject", 0),
    ] {
        let mut context = context();
        let opinion = Arc::new(pagis_mail::fake::FakeMailOpinions {
            answer: std::sync::Mutex::new(Some(stored_opinion())),
            ..Default::default()
        });
        context.intake = Arc::new(pagis_mail::MailIntake {
            opinions: opinion.clone(),
            workspaces: context.intake.workspaces.clone(),
            agents: context.intake.agents.clone(),
            source: context.intake.source.clone(),
            records: context.intake.records.clone(),
            trust: context.intake.trust.clone(),
        });
        let raw = format!(
            "From: sender@test\r\nMessage-ID: <one@test>\r\nX-Rspamd-Score: {}\r\nX-Rspamd-Action: {action}\r\n\r\nBody",
            f64::from(score) / 1000.0
        );
        let mut message = read_mime(raw.as_bytes(), MessageId::new("INBOX:1")).unwrap();
        context.assess(&mut message.summary, junk).await.unwrap();
        assert_eq!(opinion.calls.load(Ordering::SeqCst), calls);
        message.summary.assessment = None;
        context.assess(&mut message.summary, junk).await.unwrap();
        assert_eq!(opinion.calls.load(Ordering::SeqCst), calls);
    }
}

fn stored_opinion() -> pagis_core::MailOpinion {
    pagis_core::MailOpinion {
        label: pagis_core::MailOpinionLabel::Spam,
        confidence: 80,
        reasons: vec!["Unwanted offer".into()],
    }
}

async fn context_with_stored_opinion() -> (
    pagis_mail::MailReadContext,
    std::sync::Arc<pagis_mail::fake::FakeMailOpinions>,
    pagis_mail::MessageSummary,
) {
    use std::sync::Arc;
    let mut context = context();
    let opinions = Arc::new(pagis_mail::fake::FakeMailOpinions::default());
    context.intake = Arc::new(pagis_mail::MailIntake {
        opinions: opinions.clone(),
        workspaces: context.intake.workspaces.clone(),
        agents: context.intake.agents.clone(),
        source: context.intake.source.clone(),
        records: context.intake.records.clone(),
        trust: context.intake.trust.clone(),
    });
    let summary = read_mime(
        b"From: sender@test\r\nMessage-ID: <one@test>\r\nX-Rspamd-Score: 8.0\r\n\r\nBody",
        MessageId::new("INBOX:1"),
    )
    .unwrap()
    .summary;
    let mut verdict = pagis_mail::parse_spam_verdict(&summary.spam_headers, false);
    verdict.apply_opinion(stored_opinion());
    context
        .intake
        .records
        .put(
            &context.workspace,
            &context.address,
            summary.assessment_key(),
            &pagis_core::MailAssessment {
                verdict,
                trust_tier: pagis_core::TrustTier::Unknown,
            },
        )
        .await
        .unwrap();
    (context, opinions, summary)
}

#[tokio::test]
async fn stored_opinion_does_not_override_a_new_junk_or_clean_host_verdict() {
    use std::sync::atomic::Ordering;
    for (in_junk, score, action, expected_action) in [
        (true, "8.0", "no action", SpamAction::Junk),
        (false, "8.0", "reject", SpamAction::Reject),
        (false, "2.0", "no action", SpamAction::NoAction),
    ] {
        let (context, opinions, mut summary) = context_with_stored_opinion().await;
        summary.spam_headers = vec![
            ("X-Rspamd-Score".into(), score.into()),
            ("X-Rspamd-Action".into(), action.into()),
        ];
        context.assess(&mut summary, in_junk).await.unwrap();
        let verdict = &summary.assessment.as_ref().unwrap().verdict;
        assert_eq!(verdict.action, expected_action);
        assert_eq!(verdict.score, if score == "2.0" { 2000 } else { 8000 });
        assert!(verdict.opinion.is_none());
        assert_eq!(opinions.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            context
                .intake
                .records
                .get(
                    &context.workspace,
                    &context.address,
                    summary.assessment_key()
                )
                .await
                .unwrap()
                .unwrap()
                .verdict,
            *verdict
        );
    }
}

#[tokio::test]
async fn uncertain_mail_reuses_the_stored_opinion_with_the_current_host_score() {
    use std::sync::atomic::Ordering;
    let (context, opinions, mut summary) = context_with_stored_opinion().await;
    summary.spam_headers = vec![("X-Rspamd-Score".into(), "10.0".into())];
    for _ in 0..2 {
        summary.assessment = None;
        context.assess(&mut summary, false).await.unwrap();
        let verdict = &summary.assessment.as_ref().unwrap().verdict;
        assert_eq!(verdict.score, 12400);
        assert_eq!(verdict.opinion, Some(stored_opinion()));
        assert_eq!(verdict.reasons, ["model_opinion spam 80"]);
        assert_eq!(opinions.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn uncertain_mail_without_a_stored_opinion_asks_the_model_again() {
    let (context, opinions, mut summary) = context_with_stored_opinion().await;
    context
        .intake
        .records
        .put(
            &context.workspace,
            &context.address,
            summary.assessment_key(),
            &pagis_core::MailAssessment {
                verdict: pagis_mail::parse_spam_verdict(&summary.spam_headers, false),
                trust_tier: pagis_core::TrustTier::Unknown,
            },
        )
        .await
        .unwrap();
    *opinions.answer.lock().unwrap() = Some(stored_opinion());
    context.assess(&mut summary, false).await.unwrap();
    assert_eq!(opinions.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        summary.assessment.unwrap().verdict.opinion,
        Some(stored_opinion())
    );
}
