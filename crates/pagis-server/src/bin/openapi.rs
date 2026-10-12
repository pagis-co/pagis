//! Print the OpenAPI spec to stdout. `ui/` codegen and the drift check
//! consume this.

fn main() {
    println!("{}", pagis_server::ApiDoc::to_pretty_json());
}
