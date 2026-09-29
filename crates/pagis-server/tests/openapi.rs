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
