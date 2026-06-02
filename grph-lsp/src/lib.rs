pub mod convert;
pub mod handlers;
pub mod transport;

pub fn run_lsp_server(project_root: std::path::PathBuf) -> std::io::Result<()> {
    transport::serve_stdio(project_root)
}
