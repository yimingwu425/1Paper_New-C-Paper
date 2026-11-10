#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let url = url::Url::parse("http://127.0.0.1:47831/")?;
            let origin = url.origin();
            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
                .title("C-Paper")
                .inner_size(1240.0, 860.0)
                .min_inner_size(800.0, 600.0)
                .on_navigation(move |target| target.origin() == origin)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("C-Paper 桌面窗口启动失败；请使用 scripts/desktop-local.sh 启动完整服务");
}
