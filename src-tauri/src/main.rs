// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;
use transcreve_ai_app_lib::CliArgs;

fn main() {
    let cli_args = CliArgs::parse();

    // MCP owns stdout. Branch before logs, Tauri, single-instance, audio and windows.
    if cli_args.connectors_mcp {
        let result = match cli_args.mcp_client_id.as_deref() {
            Some(client_id) => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "MCP runtime unavailable".to_string())
                .and_then(|runtime| {
                    runtime.block_on(transcreve_ai_app_lib::run_connectors_mcp(client_id))
                }),
            None => Err("MCP client is not paired".to_string()),
        };
        if result.is_err() {
            eprintln!(
                "Connectors MCP unavailable. Open Transcreve.ai and check the paired connection."
            );
            std::process::exit(1);
        }
        return;
    }

    #[cfg(target_os = "linux")]
    {
        // DMABUF renderer causes crashes on various GPU/display server configurations
        // See: https://github.com/tauri-apps/tauri/issues/9394
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    #[cfg(target_os = "windows")]
    {
        // Avoid overlay/capture layer crashes (#2049). Set before backend
        // initialization, preserving user overrides.
        if std::env::var_os("VK_LOADER_LAYERS_DISABLE").is_none()
            && !transcreve_ai_app_lib::env_flag_enabled("TRANSCREVE_KEEP_VULKAN_IMPLICIT_LAYERS")
        {
            std::env::set_var("VK_LOADER_LAYERS_DISABLE", "~implicit~");
        }
    }

    transcreve_ai_app_lib::run(cli_args)
}
