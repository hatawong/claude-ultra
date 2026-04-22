use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager,
};

use crate::GatewayServiceState;

pub fn create_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    // Colored tray icon (like Clash Verge style)
    let icon = Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

    // Menu items
    let status_item = MenuItem::with_id(app, "status", "Proxy: Starting...", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let show_item = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[
        &status_item,
        &sep1,
        &show_item,
        &sep2,
        &quit_item,
    ])?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)  // macOS template image: system handles color
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "show" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                        #[cfg(target_os = "macos")]
                        app.set_activation_policy(tauri::ActivationPolicy::Regular)
                            .unwrap_or(());
                    }
                }
                "quit" => {
                    // Stop proxy gracefully before quit
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let state = handle.state::<GatewayServiceState>();
                        let mut instance = state.instance.write().await;
                        if let Some(proxy) = instance.take() {
                            proxy.stop().await;
                        }
                        // Small delay for socket cleanup
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        handle.exit(0);
                    });
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            // Left click opens window
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    #[cfg(target_os = "macos")]
                    app.set_activation_policy(tauri::ActivationPolicy::Regular)
                        .unwrap_or(());
                }
            }
        })
        .build(app)?;

    // Update status after proxy starts
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Wait a bit for proxy to start
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        update_tray_status(&handle).await;
    });

    Ok(())
}

pub async fn update_tray_status(app: &tauri::AppHandle) {
    let state = app.state::<GatewayServiceState>();
    let instance = state.instance.read().await;

    let status_text = if let Some(ref proxy) = *instance {
        let count = state.client_manager.available_count();
        format!("Proxy: :{} ({} accounts)", proxy.port, count)
    } else {
        "Proxy: Not running".to_string()
    };

    if let Some(tray) = app.tray_by_id("main") {
        // Rebuild menu with updated status
        let status_item = MenuItem::with_id(app, "status", &status_text, false, None::<&str>);
        let show_item = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>);
        let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>);

        if let (Ok(s), Ok(show), Ok(quit)) = (status_item, show_item, quit_item) {
            let sep1 = PredefinedMenuItem::separator(app).ok();
            let sep2 = PredefinedMenuItem::separator(app).ok();

            let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![&s];
            if let Some(ref sep) = sep1 { items.push(sep); }
            items.push(&show);
            if let Some(ref sep) = sep2 { items.push(sep); }
            items.push(&quit);

            if let Ok(menu) = Menu::with_items(app, &items) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    }
}
