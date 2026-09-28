use serde::Serialize;
#[cfg(target_os = "macos")]
mod native_data;
#[cfg(target_os = "macos")]
use tauri::Manager;

/// Host facts, not a promise that an unimplemented native adapter is available.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeCapabilities {
    platform: &'static str,
    native_media: bool,
    remote_input: bool,
    browser_validation_only: bool,
    native_data_channels: bool,
}

#[tauri::command]
fn native_capabilities() -> NativeCapabilities {
    NativeCapabilities {
        platform: std::env::consts::OS,
        native_media: false,
        remote_input: false,
        browser_validation_only: true,
        native_data_channels: cfg!(target_os = "macos"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder
        .manage(native_data::NativeDataHost::default())
        .invoke_handler(tauri::generate_handler![
            native_capabilities,
            native_data::native_data_document,
            native_data::native_data_open,
            native_data::native_data_read,
            native_data::native_data_join,
            native_data::native_data_send,
            native_data::native_data_leave,
            native_data::native_data_destroy,
        ])
        .on_page_load(|webview, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
                let host = webview.state::<native_data::NativeDataHost>();
                if let Err(error) =
                    tauri::async_runtime::block_on(host.reset_document(webview.label()))
                {
                    eprintln!("Native document cleanup failed: {}", error.code);
                }
            }
        })
        .on_web_content_process_terminate(|webview| {
            let host = webview.state::<native_data::NativeDataHost>();
            if let Err(error) = tauri::async_runtime::block_on(host.reset_document(webview.label()))
            {
                eprintln!("Native renderer cleanup failed: {}", error.code);
            }
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let host = window.state::<native_data::NativeDataHost>();
                // These data-only resources use native engine/Tokio threads, not the UI event loop.
                if let Err(error) =
                    tauri::async_runtime::block_on(host.close_window(window.label()))
                {
                    eprintln!("Native host cleanup failed: {}", error.code);
                }
            }
        });
    #[cfg(not(target_os = "macos"))]
    let builder = builder.invoke_handler(tauri::generate_handler![native_capabilities]);
    let app = builder
        .build(tauri::generate_context!())
        .expect("desktop host failed to start");
    app.run(|_app, _event| {
        #[cfg(target_os = "macos")]
        if matches!(_event, tauri::RunEvent::Exit) {
            if let Err(error) = tauri::async_runtime::block_on(
                _app.state::<native_data::NativeDataHost>()
                    .close_window("main"),
            ) {
                eprintln!("Native host shutdown failed: {}", error.code);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::native_capabilities;

    #[test]
    fn unfinished_adapters_are_never_reported_as_supported() {
        let capabilities = native_capabilities();
        assert!(!capabilities.native_media);
        assert!(!capabilities.remote_input);
        assert!(capabilities.browser_validation_only);
        assert_eq!(capabilities.native_data_channels, cfg!(target_os = "macos"));
    }
}
