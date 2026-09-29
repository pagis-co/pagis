//! Print the OpenAPI spec to stdout. `ui/` codegen and the drift check
//! consume this.

use utoipa::OpenApi;

fn main() {
    println!(
        "{}",
        pagis_server::ApiDoc::openapi()
            .to_pretty_json()
            .expect("spec serializes")
    );
}
