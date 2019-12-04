/***
 * This file is part of Poki Launcher.
 *
 * Poki Launcher is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * Poki Launcher is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with Poki Launcher.  If not, see <https://www.gnu.org/licenses/>.
 */
use cstr::*;
use failure::Error;
use lazy_static::lazy_static;
use lib_poki_launcher::prelude::*;
use log::{error, trace};
use poki_launcher_notifier::{self as notifier, Notifier};
use poki_launcher_x11::foreground;
use qmetaobject::*;
use std::cell::{Cell, RefCell};
use std::convert::From;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

const MAX_APPS_SHOWN: usize = 5;

pub fn log_errs(errs: &[Error]) {
    for err in errs {
        error!("{}", err);
    }
}

lazy_static! {
    pub static ref DB_PATH: PathBuf = {
        use std::fs::create_dir;
        let data_dir = DIRS.data_dir();
        if !data_dir.exists() {
            create_dir(&data_dir).unwrap_or_else(|_| {
                panic!("Failed to create data dir: {}", data_dir.to_string_lossy())
            });
        }
        let mut db_file = data_dir.to_path_buf();
        db_file.push("apps.db");
        db_file
    };
    pub static ref APPS: Arc<Mutex<Option<AppsDB>>> = Arc::new(Mutex::new(None));
}

thread_local! {
    pub static CONF: Config = Config::load().unwrap();
    pub static SHOW_ON_START: Cell<bool> = Cell::new(true);
}

#[derive(QObject, Default)]
struct PokiLauncher {
    base: qt_base_class!(trait QObject),
    list: Vec<App>,
    model: qt_property!(RefCell<SimpleListModel<QApp>>; NOTIFY model_changed),
    selected: qt_property!(String; NOTIFY selected_changed WRITE set_selected),
    visible: qt_property!(bool; NOTIFY visible_changed),
    scanning: qt_property!(bool; NOTIFY scanning_changed),

    init: qt_method!(fn(&mut self)),
    search: qt_method!(fn(&mut self, text: String)),
    scan: qt_method!(fn(&mut self)),
    down: qt_method!(fn(&mut self)),
    up: qt_method!(fn(&mut self)),
    run: qt_method!(fn(&mut self)),
    hide: qt_method!(fn(&mut self)),
    exit: qt_method!(fn(&mut self)),

    selected_changed: qt_signal!(),
    visible_changed: qt_signal!(),
    scanning_changed: qt_signal!(),
    model_changed: qt_signal!(),
}

impl PokiLauncher {
    fn init(&mut self) {
        self.visible = SHOW_ON_START.with(|b| b.get());
        self.visible_changed();
        let rx = Notifier::start().unwrap();
        let qptr = QPointer::from(&*self);
        let show = qmetaobject::queued_callback(move |()| {
            qptr.as_pinned().map(|self_| {
                self_.borrow_mut().visible = true;
                self_.borrow().visible_changed();
            });
        });
        thread::spawn(move || loop {
            use notifier::Msg;
            match rx.recv().unwrap() {
                Msg::Show => {
                    show(());
                    foreground("Poki Launcher");
                }
                Msg::Exit => {
                    drop(rx);
                    std::process::exit(0);
                }
            }
        });
    }

    fn set_selected<T: Into<QString>>(&mut self, selected: T) {
        self.selected = selected.into().into();
        self.selected_changed();
    }

    fn search(&mut self, text: String) {
        let lock = APPS.lock().expect("Apps Mutex Poisoned");
        self.list = lock
            .as_ref()
            .unwrap()
            .get_ranked_list(&text, Some(MAX_APPS_SHOWN));
        if !self.list.is_empty() {
            self.selected = self.list[0].uuid.clone().into();
        } else {
            self.selected = Default::default();
        }
        self.model
            .borrow_mut()
            .reset_data(self.list.clone().into_iter().map(QApp::from).collect());
        self.selected_changed();
    }

    fn scan(&mut self) {
        trace!("Scanning...");
        self.scanning = true;
        self.scanning_changed();
        let config = CONF.with(|c| c.clone());
        let qptr = QPointer::from(&*self);
        let done = qmetaobject::queued_callback(move |()| {
            qptr.as_pinned().map(|self_| {
                self_.borrow_mut().scanning = false;
                self_.borrow().scanning_changed();
            });
        });
        thread::spawn(move || {
            let (app_list, errors) = scan_desktop_entries(&config.app_paths);
            let mut lock = APPS.lock().expect("Apps Mutex Poisoned");
            match *lock {
                Some(ref mut apps) => {
                    apps.merge_new_entries(app_list);
                    if let Err(e) = apps.save(&*DB_PATH) {
                        error!("Saving database failed: {}", e);
                    }
                }
                None => panic!("APPS Not init"),
            }
            log_errs(&errors);
            done(());
            trace!("Scanning...done");
        });
    }

    fn down(&mut self) {
        if self.list.is_empty() {
            return;
        }
        let (idx, _) = self
            .list
            .iter()
            .enumerate()
            .find(|(_, app)| app.uuid == self.selected)
            .unwrap();
        if idx >= self.list.len() - 1 {
            self.selected = self.list[self.list.len() - 1].uuid.clone().into();
        } else {
            self.selected = self.list[idx + 1].uuid.clone().into();
        }
        self.selected_changed();
    }

    fn up(&mut self) {
        if self.list.is_empty() {
            return;
        }
        let (idx, _) = self
            .list
            .iter()
            .enumerate()
            .find(|(_, app)| app.uuid == self.selected)
            .unwrap();
        if idx == 0 {
            self.selected = self.list[0].uuid.clone();
        } else {
            self.selected = self.list[idx - 1].uuid.clone();
        }
        self.selected_changed();
    }

    fn run(&mut self) {
        if self.list.is_empty() {
            return;
        }
        let selected: String = self.selected.clone().into();
        let app = self.list.iter().find(|app| app.uuid == selected).unwrap();
        if let Err(err) = app.run() {
            error!("{}", err);
        }
        let mut lock = APPS.lock().expect("Apps Mutex Poisoned");
        match *lock {
            Some(ref mut apps) => {
                apps.update(app);
                apps.save(&*DB_PATH).unwrap();
            }
            None => panic!("APPS Not init"),
        }
        self.list.clear();
        self.model.borrow_mut().reset_data(Default::default());
        self.selected = Default::default();
        self.selected_changed();
    }

    fn hide(&mut self) {
        self.visible = false;
        self.visible_changed();
    }

    fn exit(&mut self) {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        if let Err(e) = kill(Pid::this(), Signal::SIGINT) {
            error!("Failed to signal self to exit: {}", e);
        }
    }
}

// impl Default for PokiLauncher {
//     fn default() -> Self {
//         PokiLauncher {
//             visible: true,
//             base: Default::default(),
//             list: Default::default(),
//             model: Default::default(),
//             selected: Default::default(),
//             init: Default::default(),
//             scanning: Default::default(),
//             scanning_changed: Default::default(),
//             visible_changed: Default::default(),
//             selected_changed: Default::default(),
//             search: Default::default(),
//             down: Default::default(),
//             up: Default::default(),
//             scan: Default::default(),
//             get_icon: Default::default(),
//             hide: Default::default(),
//             run: Default::default(),
//             exit: Default::default(),
//         }
//     }
// }

#[derive(Default, Clone, SimpleListItem)]
struct QApp {
    pub name: String,
    pub uuid: String,
    pub icon: String,
}

impl From<App> for QApp {
    fn from(app: App) -> QApp {
        QApp {
            name: app.name,
            uuid: app.uuid,
            icon: app.icon,
        }
    }
}

impl QMetaType for QApp {}

qrc!(init_qml_resources,
    "ui" {
        "ui/main.qml" as "main.qml",
        "ui/MainForm.ui.qml" as "MainForm.ui.qml",
    }
);

pub fn init_ui() {
    init_qml_resources();
    qml_register_type::<PokiLauncher>(cstr!("PokiLauncher"), 1, 0, cstr!("PokiLauncher"));
}