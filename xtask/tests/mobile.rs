//! Tests for the build configuration of the Mobile App in `mobile/`.

use std::process::Command;

use crate::support::workspace_root;

/// A build of the iOS project takes `PUSH_RELAY_ORIGIN` from
/// `mobile/ios/app.xcconfig`, which holds the placeholder, and from the
/// local file `mobile/ios/local.xcconfig`, which overrides it. A value in
/// **Build Settings** in Xcode goes into `project.pbxproj`. It then
/// overrides each xcconfig file and puts the origin in the repository.
#[test]
fn the_ios_push_relay_origin_of_a_local_build_stays_out_of_the_repository() {
    let root = workspace_root();
    let project =
        std::fs::read_to_string(root.join("mobile/ios/App/App.xcodeproj/project.pbxproj")).unwrap();
    assert!(
        !project.contains("PUSH_RELAY_ORIGIN = "),
        "project.pbxproj sets PUSH_RELAY_ORIGIN, which overrides \
         mobile/ios/local.xcconfig; set it in the xcconfig files instead"
    );

    let ignored = Command::new("git")
        .args(["check-ignore", "--quiet", "mobile/ios/local.xcconfig"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(
        ignored.success(),
        "git does not ignore mobile/ios/local.xcconfig"
    );
}
