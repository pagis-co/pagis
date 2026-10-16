use pagis_core::{MailVisibility, SharedWorkspaceLimits};

#[test]
fn visibility_is_ordered_and_bounded() {
    let mut limits = SharedWorkspaceLimits::default();
    for maximum in [
        MailVisibility::AgentOnly,
        MailVisibility::Escalations,
        MailVisibility::AllMail,
    ] {
        limits.max_mail_visibility = maximum;
        for selected in [
            MailVisibility::AgentOnly,
            MailVisibility::Escalations,
            MailVisibility::AllMail,
        ] {
            assert_eq!(
                limits.check_mail_visibility(selected).is_ok(),
                selected <= maximum
            );
        }
    }
}

#[test]
fn absent_lists_allow_all_and_empty_lists_allow_none() {
    let mut limits = SharedWorkspaceLimits::default();
    assert!(limits.check_resource("plugin", "one").is_ok());
    assert!(limits.check_resource("connection", "one").is_ok());
    limits.permitted_plugins = Some(vec![]);
    limits.permitted_connections = Some(vec!["two".into()]);
    assert!(limits.check_resource("plugin", "one").is_err());
    assert!(limits.check_resource("connection", "one").is_err());
    assert!(limits.check_resource("connection", "two").is_ok());
}

#[test]
fn permission_ids_must_be_valid() {
    assert!(
        SharedWorkspaceLimits {
            permitted_plugins: Some(vec![" ".into()]),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(SharedWorkspaceLimits::default().validate().is_ok());
}
