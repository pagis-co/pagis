use utoipa::OpenApi;

#[test]
fn run_state_contract_includes_reflecting() {
    let document = serde_json::to_value(pagis_server::ApiDoc::openapi()).unwrap();
    assert_eq!(
        document["components"]["schemas"]["RunStateDto"]["enum"],
        serde_json::json!([
            "queued",
            "running",
            "reflecting",
            "waiting_for_user",
            "waiting_for_approval",
            "completed",
            "failed",
            "canceled"
        ])
    );
    assert_eq!(
        document["components"]["schemas"]["RunDto"]["properties"]["state"]["$ref"],
        "#/components/schemas/RunStateDto"
    );
}

#[test]
fn plugins_contract_holds_every_route_and_schema() {
    let document = serde_json::to_value(pagis_server::ApiDoc::openapi()).unwrap();
    for route in pagis_server::ROUTES
        .iter()
        .chain(pagis_server::ADMINISTRATION_ROUTES)
        .filter(|route| {
            ["/api/v1/plugins", "/api/v1/software", "/api/v1/widgets"]
                .iter()
                .any(|prefix| route.path.starts_with(prefix))
        })
    {
        for method in route.methods {
            assert!(
                document["paths"][route.path][method].is_object(),
                "{} {method} has an OpenAPI operation",
                route.path
            );
        }
    }
    for schema in [
        "PluginPage",
        "PluginDto",
        "InstallPluginRequest",
        "UpdatePluginRequest",
        "BindingValueRequest",
        "GrantPluginRequest",
        "PluginLogDto",
        "SoftwarePackagePage",
        "SoftwarePackageDetailDto",
        "ContributionDto",
        "WidgetViewDto",
        "JsonRpcRequest",
        "JsonRpcResponse",
    ] {
        assert!(
            document["components"]["schemas"][schema].is_object(),
            "{schema} has an OpenAPI schema"
        );
    }
}

#[test]
fn connections_contract_keeps_the_settings_and_providers_tags() {
    let document = serde_json::to_value(pagis_server::ApiDoc::openapi()).unwrap();
    for (path, methods, tag) in [
        (
            "/api/v1/settings/connections/providers",
            vec!["get"],
            "settings",
        ),
        (
            "/api/v1/settings/connections",
            vec!["get", "post"],
            "settings",
        ),
        (
            "/api/v1/settings/connections/{connection_id}",
            vec!["delete"],
            "settings",
        ),
        (
            "/api/v1/settings/connections/{connection_id}/authorize",
            vec!["post"],
            "settings",
        ),
        ("/api/v1/connections/google/start", vec!["get"], "settings"),
        (
            "/api/v1/connections/google/callback",
            vec!["get"],
            "settings",
        ),
        ("/api/v1/administration/providers", vec!["get"], "providers"),
        (
            "/api/v1/administration/providers/{provider}/{part}",
            vec!["put", "delete"],
            "providers",
        ),
        (
            "/api/v1/administration/providers/{provider}/{part}/test",
            vec!["post"],
            "providers",
        ),
    ] {
        for method in methods {
            assert_eq!(
                document["paths"][path][method]["tags"],
                serde_json::json!([tag]),
                "{method} {path}"
            );
        }
    }
}
