use fuzzy_matcher::skim::fuzzy_match;
use launcher::runner::*;
use launcher::scan::*;
use launcher::{self, App};

use rmp_serde as rmp;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use std::sync::mpsc;
use std::thread;
use gdk::enums::key;

use gtk::prelude::*;
use gio::prelude::*;
use glib::{self, signal::Inhibit};
use gtk::{Application, ApplicationWindow, Entry, EntryExt,
    WidgetExt, BoxExt, Label, LabelExt, TreeView, TreeViewExt,
    TreeStore, TreeStoreExt, TreeViewColumn, CellRendererText};


const DB_PATH: &'static str = "apps.db";

#[derive(Debug, Clone)]
enum InMsg {
    SearchText(String),
    Run,
    Exit,
}

#[derive(Debug, Clone)]
enum OutMsg {
    AppList(Vec<App>),
    Hide,
}

fn build_ui(application: &gtk::Application, apps: Vec<App>) {
    let (input_tx, input_rx): (mpsc::Sender<InMsg>, mpsc::Receiver<InMsg>) = mpsc::channel();
    let (output_tx, output_rx): (glib::Sender<OutMsg>, glib::Receiver<OutMsg>) =
        glib::MainContext::channel(glib::PRIORITY_HIGH);

    thread::spawn(move || {
        let mut to_launch = None;
        loop {
            match input_rx.recv().unwrap() {
                InMsg::SearchText(text) => {
                    let mut app_list = apps
                        .iter()
                        .filter_map(|app| match fuzzy_match(&app.name, &text) {
                            Some(score) if score > 0 => Some((app.clone(), score)),
                            _ => None,
                        })
                        .collect::<Vec<(App, i64)>>();
                    app_list.sort_by(|left, right| right.1.cmp(&left.1));
                    let ret_list: Vec<App> = app_list.into_iter().map(|(app, _)| app).collect();
                    if let Some(app) = ret_list.get(0) {
                        to_launch = Some(app.clone());
                    } else {
                        to_launch = None;
                    }
                    output_tx.send(OutMsg::AppList(ret_list)).unwrap();
                }
                InMsg::Run => {
                    if let Some(app) = &to_launch {
                        // TODO Handle app run failures
                        app.run().unwrap();
                        output_tx.send(OutMsg::Hide).unwrap();
                    }
                }
                InMsg::Exit => {
                    return;
                }
            }
        }
    });

    let window = ApplicationWindow::new(application);
    let top_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let entry = Entry::new();
    let tree = TreeView::new();
    let column_types = [String::static_type()];
    let store = TreeStore::new(&column_types);
    let col = TreeViewColumn::new();
    let renderer = CellRendererText::new();
    col.pack_start(&renderer, true);
    col.add_attribute(&renderer, "text", 0);
    tree.append_column(&col);
    tree.set_model(Some(&store));
    tree.set_headers_visible(false);
    // store.insert_with_values(None, None, &[0], &[&"App Oh!"]);

    window.set_title("Poki Launcher");
    window.set_default_size(350, 70);
    window.set_position(gtk::WindowPosition::Center);

    top_box.pack_start(&entry, true, true, 0);
    top_box.pack_end(&tree, true, true, 0);
    window.add(&top_box);
    let search_tx = input_tx.clone();
    entry.connect_changed(move |entry| {
        dbg!(&entry);
        if let Some(text) = entry.get_text() {
            let text_str = text.as_str().to_owned();
            search_tx
                .send(InMsg::SearchText(text_str))
                .expect("Failed to send search text to other thread");
        }
    });
    let run_tx = input_tx.clone();
    entry.connect_key_press_event(move |entry, event| {
        if event.get_keyval() == key::Return {
            println!("Enter pressed!");
            run_tx.send(InMsg::Run).unwrap();
        }
        Inhibit(false)
    });

    output_rx.attach(None, move |msg| {
        match msg {
            OutMsg::AppList(apps) => {
                store.clear();
                println!("--------------------------");
                for app in &apps {
                    println!("{}", app);
                    store.insert_with_values(None, None, &[0], &[&app.name]);
                }
            }
            OutMsg::Hide => {}
        }
        glib::Continue(true)
    });

    // entry.show();

    window.show_all();
    window.present();
    window.set_keep_above(true);
}

// if let Some(app) = app_list.get(0) {
//     *to_launch.borrow_mut() = Some(app.0.clone());
// }
fn main() {
    let application = Application::new("info.bengoldberg.poki_launcher", Default::default())
        .expect("failed to initialize GTK application");

    application.connect_activate(|app| {
        let db_path = Path::new(&DB_PATH);
        let apps: Vec<App> = if db_path.exists() {
            let mut apps_file = File::open(&db_path).unwrap();
            let mut buf = Vec::new();
            apps_file.read_to_end(&mut buf).unwrap();
            let mut de = rmp::Deserializer::new(&buf[..]);
            Deserialize::deserialize(&mut de).unwrap()
        } else {
            let desktop_files = desktop_files();
            let desktop_files = desktop_files.unwrap();
            let (apps, _errs) = parse_parse_entries(desktop_files);
            let mut buf = Vec::new();
            apps.serialize(&mut rmp::Serializer::new(&mut buf)).unwrap();
            let mut file = File::create("apps.db").unwrap();
            file.write_all(&buf).unwrap();
            apps
        };

        build_ui(app, apps);
    });

    application.run(&[]);
}
