use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label};

fn main() {
    let app = Application::builder()
        .application_id("org.kalam.engine.demo")
        .build();

    app.connect_activate(|app| {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Kalam Engine Demo")
            .default_width(800)
            .default_height(600)
            .build();

        let label = Label::new(Some("kalam-engine: Pure-Rust Text Renderer"));
        window.set_child(Some(&label));
        window.present();
    });

    app.run();
}
