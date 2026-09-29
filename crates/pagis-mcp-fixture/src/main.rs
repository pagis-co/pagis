//! The fixture MCP server as one command.
//!
//! With no argument it speaks stdio, which is how a plugin's stdio
//! server runs. With `--http <port>` it serves streamable HTTP on the
//! loopback interface at `/mcp`.

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [] => pagis_mcp_fixture::serve_stdio().await,
        [flag, port] if flag == "--http" => {
            let address = format!("127.0.0.1:{port}");
            let listener = tokio::net::TcpListener::bind(&address).await?;
            axum::serve(listener, pagis_mcp_fixture::http_router()).await
        }
        _ => Err(std::io::Error::other(
            "usage: pagis-mcp-fixture [--http <port>]",
        )),
    }
}
