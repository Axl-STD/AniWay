//! Test client: a window that starts a drag carrying the files given as arguments (stand-in for a file manager).
use gtk::prelude::*;
use gtk::{gdk, gio};

fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder().application_id("io.github.axl_std.AniWay.DragSource").build();
    app.connect_activate(|app| {
        let files: Vec<gio::File> = std::env::args().skip(1).map(gio::File::for_path).collect();
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::COPY);
        source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value()))
        });
        let label = gtk::Label::new(Some("drag from here"));
        label.add_controller(source);
        gtk::ApplicationWindow::builder().application(app).child(&label).build().present();
    });
    app.run_with_args::<&str>(&[])
}
