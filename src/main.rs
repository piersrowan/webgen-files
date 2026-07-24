//! WebGen Files — a native GTK4/libadwaita file manager for the WebGen family.
//!
//! Left: a collapsible folder tree ([`tree`]). Right: the current folder's files + subfolders with
//! icons ([`entry`]). New Folder, Copy/Paste, Open, Open With…, and a filter/search box scoped to
//! the current folder (with a Recursive option). "Open" honours the shared "Opens With" file
//! associations ([`assoc`]), which System Settings edits too.

mod assoc;
mod entry;
mod meta;
mod tree;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::clone;
use gtk::glib::BoxedAnyObject;
use gtk::{gdk, gio, glib};

/// Opens the right-click context menu for a row (anchor widget + click coords). Set once the menu
/// model exists; the per-row gesture (built earlier, in the factory) calls through this holder.
type MenuOpener = Rc<RefCell<Option<Rc<dyn Fn(gtk::Widget, f64, f64)>>>>;

use entry::Entry;
use webgen_registry::Registry;

const APP_ID: &str = "com.webgen.Files";
const FILES_NS: &str = "com.webgen.Files";

type Reg = Option<Rc<Registry>>;

fn show_hidden(reg: &Reg) -> bool {
    reg.as_ref()
        .map(|r| r.get_bool(FILES_NS, "show_hidden", false))
        .unwrap_or(false)
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let reg: Reg = Registry::open_default().ok().map(Rc::new);
    if let Some(r) = &reg {
        assoc::seed_defaults(r);
    }

    let start = glib::home_dir();
    let current = Rc::new(RefCell::new(start.clone()));
    let history = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    let forward = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    let clipboard = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    // true = the clipboard holds a "cut" (paste moves); false = a "copy" (paste copies).
    let cut_mode = Rc::new(Cell::new(false));
    let query = Rc::new(RefCell::new(String::new()));
    let recursive = Rc::new(Cell::new(false));

    // ---- right-hand file list: ListStore<Entry> -> filter -> multi-selection -> ListView -------
    let store = gio::ListStore::new::<BoxedAnyObject>();
    let filter = gtk::CustomFilter::new(clone!(
        #[strong] query,
        #[strong] recursive,
        move |obj| {
            if recursive.get() {
                return true; // recursive results are already only matches
            }
            let q = query.borrow().to_lowercase();
            if q.is_empty() {
                return true;
            }
            let b = obj.downcast_ref::<BoxedAnyObject>().unwrap();
            let e = b.borrow::<Entry>();
            e.name.to_lowercase().contains(&q)
        }
    ));
    let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
    let selection = gtk::MultiSelection::new(Some(filtered));
    let menu_opener: MenuOpener = Rc::new(RefCell::new(None));

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(clone!(
        #[strong] selection,
        #[strong] menu_opener,
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.set_margin_top(3);
            row.set_margin_bottom(3);
            let icon = gtk::Image::new();
            icon.set_pixel_size(24);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let name = gtk::Label::new(None);
            name.set_xalign(0.0);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let sub = gtk::Label::new(None);
            sub.set_xalign(0.0);
            sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
            sub.add_css_class("dim-label");
            sub.add_css_class("caption");
            text.append(&name);
            text.append(&sub);
            row.append(&icon);
            row.append(&text);
            item.set_child(Some(&row));

            // Right-click: select this row (unless it's already in a multi-selection) then open the
            // context menu at the pointer.
            let gesture = gtk::GestureClick::new();
            gesture.set_button(gdk::BUTTON_SECONDARY);
            let li = item.clone();
            gesture.connect_pressed(clone!(
                #[weak] row,
                #[weak] selection,
                #[strong] menu_opener,
                move |_g, _n, x, y| {
                    let pos = li.position();
                    if pos != gtk::INVALID_LIST_POSITION && !selection.is_selected(pos) {
                        selection.select_item(pos, true);
                    }
                    // Anchor the menu to the *ListView*, not this row: selecting the item above can
                    // re-render and unparent the clicked row, and on Wayland an xdg-popup whose
                    // parent surface has gone silently fails to map (the menu never appears). The
                    // ListView is always mapped. Translate the click into its coordinate space so
                    // the menu still points at the cursor.
                    let anchor = row
                        .ancestor(gtk::ListView::static_type())
                        .unwrap_or_else(|| row.clone().upcast::<gtk::Widget>());
                    let (ax, ay) = row.translate_coordinates(&anchor, x, y).unwrap_or((x, y));
                    // Defer the popup to idle: popping up synchronously inside the button-PRESS
                    // handler lets the following RELEASE dismiss it before it shows.
                    let opener = menu_opener.clone();
                    glib::idle_add_local_once(move || {
                        if let Some(open) = opener.borrow().as_ref() {
                            open(anchor, ax, ay);
                        }
                    });
                }
            ));
            row.add_controller(gesture);
        }
    ));
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let b = item.item().and_downcast::<BoxedAnyObject>().unwrap();
        let e = b.borrow::<Entry>();
        let row = item.child().and_downcast::<gtk::Box>().unwrap();
        let icon = row.first_child().and_downcast::<gtk::Image>().unwrap();
        let text = row.last_child().and_downcast::<gtk::Box>().unwrap();
        let name = text.first_child().and_downcast::<gtk::Label>().unwrap();
        let sub = text.last_child().and_downcast::<gtk::Label>().unwrap();

        match &e.icon {
            Some(gicon) => icon.set_from_gicon(gicon),
            None => icon.set_icon_name(Some(if e.is_dir { "folder" } else { "text-x-generic" })),
        }
        name.set_text(&e.name);
        if e.subtitle.is_empty() {
            sub.set_visible(false);
        } else {
            sub.set_visible(true);
            sub.set_text(&e.subtitle);
        }
    });

    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.add_css_class("navigation-sidebar");
    let list_scroller = gtk::ScrolledWindow::new();
    list_scroller.set_hexpand(true);
    list_scroller.set_vexpand(true);
    list_scroller.set_child(Some(&list));

    // ---- header / toolbar widgets --------------------------------------------------------------
    let btn_back = gtk::Button::from_icon_name("go-previous-symbolic");
    btn_back.set_tooltip_text(Some("Back"));
    let btn_forward = gtk::Button::from_icon_name("go-next-symbolic");
    btn_forward.set_tooltip_text(Some("Forward"));
    let btn_up = gtk::Button::from_icon_name("go-up-symbolic");
    btn_up.set_tooltip_text(Some("Up"));
    let btn_tree = gtk::ToggleButton::new();
    btn_tree.set_icon_name("view-list-symbolic");
    btn_tree.set_tooltip_text(Some("Show folder tree"));
    btn_tree.set_active(true);

    let path_entry = gtk::Entry::new();
    path_entry.set_hexpand(true);
    path_entry.set_primary_icon_name(Some("folder-symbolic"));

    let btn_new = gtk::Button::from_icon_name("folder-new-symbolic");
    btn_new.set_tooltip_text(Some("New Folder"));
    let btn_copy = gtk::Button::from_icon_name("edit-copy-symbolic");
    btn_copy.set_tooltip_text(Some("Copy"));
    let btn_paste = gtk::Button::from_icon_name("edit-paste-symbolic");
    btn_paste.set_tooltip_text(Some("Paste"));
    let btn_openwith = gtk::Button::from_icon_name("emblem-system-symbolic");
    btn_openwith.set_tooltip_text(Some("Open With…"));

    let search = gtk::SearchEntry::new();
    search.set_hexpand(true);
    search.set_placeholder_text(Some("Filter this folder"));
    let chk_recursive = gtk::CheckButton::with_label("Recursive");
    chk_recursive.set_tooltip_text(Some("Search all subfolders below the current folder"));

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("dim-label");

    // ---- reload: rebuild the file list for the current folder + query -------------------------
    let reload: Rc<dyn Fn()> = {
        let (store, current, reg, query, recursive) =
            (store.clone(), current.clone(), reg.clone(), query.clone(), recursive.clone());
        let (filter, path_entry, status) = (filter.clone(), path_entry.clone(), status.clone());
        Rc::new(move || {
            store.remove_all();
            let dir = current.borrow().clone();
            let hidden = show_hidden(&reg);
            let q = query.borrow().clone();
            let mut capped = false;
            if recursive.get() && !q.trim().is_empty() {
                let (entries, was_capped) = entry::search(&dir, q.trim(), hidden);
                capped = was_capped;
                for e in entries {
                    store.append(&BoxedAnyObject::new(e));
                }
            } else {
                for e in entry::list_dir(&dir, hidden) {
                    store.append(&BoxedAnyObject::new(e));
                }
            }
            filter.changed(gtk::FilterChange::Different);
            path_entry.set_text(&dir.to_string_lossy());

            let shown = if recursive.get() && !q.trim().is_empty() {
                store.n_items()
            } else {
                // count what passes the substring filter
                let ql = q.to_lowercase();
                (0..store.n_items())
                    .filter(|&i| {
                        ql.is_empty()
                            || store
                                .item(i)
                                .and_downcast::<BoxedAnyObject>()
                                .map(|b| b.borrow::<Entry>().name.to_lowercase().contains(&ql))
                                .unwrap_or(false)
                    })
                    .count() as u32
            };
            let noun = if shown == 1 { "item" } else { "items" };
            status.set_text(&if recursive.get() && !q.trim().is_empty() {
                format!("{shown} results{}", if capped { " (showing the first 2000)" } else { "" })
            } else {
                format!("{shown} {noun}")
            });
        })
    };

    // ---- navigate: change folder (records history) --------------------------------------------
    let navigate: Rc<dyn Fn(PathBuf)> = {
        let (current, history, forward, query, search, reload) = (
            current.clone(),
            history.clone(),
            forward.clone(),
            query.clone(),
            search.clone(),
            reload.clone(),
        );
        Rc::new(move |path: PathBuf| {
            if !path.is_dir() {
                return;
            }
            {
                let mut cur = current.borrow_mut();
                if *cur != path {
                    history.borrow_mut().push(cur.clone());
                    forward.borrow_mut().clear();
                    *cur = path;
                }
            }
            query.borrow_mut().clear();
            search.set_text(""); // fires search handler (reload); explicit reload covers empty case
            reload();
        })
    };

    // ---- window (needed as dialog parent) ------------------------------------------------------
    let tree_pane = tree::build(
        show_hidden(&reg),
        clone!(#[strong] navigate, move |p| navigate(p)),
    );
    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&tree_pane));
    split.set_end_child(Some(&list_scroller));
    split.set_resize_start_child(false);
    split.set_shrink_start_child(false);
    split.set_position(240);

    let header = adw::HeaderBar::new();
    let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    nav_box.add_css_class("linked");
    nav_box.append(&btn_back);
    nav_box.append(&btn_forward);
    nav_box.append(&btn_up);
    header.pack_start(&nav_box);
    header.pack_start(&btn_tree);
    header.set_title_widget(Some(&path_entry));
    header.pack_end(&btn_new);
    header.pack_end(&btn_openwith);
    header.pack_end(&btn_paste);
    header.pack_end(&btn_copy);

    let search_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    search_bar.set_margin_start(8);
    search_bar.set_margin_end(8);
    search_bar.set_margin_top(6);
    search_bar.set_margin_bottom(6);
    search_bar.append(&search);
    search_bar.append(&chk_recursive);

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.set_margin_start(12);
    bottom.set_margin_end(12);
    bottom.set_margin_top(4);
    bottom.set_margin_bottom(4);
    bottom.append(&status);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&search_bar);
    toolbar.set_content(Some(&split));
    toolbar.add_bottom_bar(&bottom);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("WebGen Files")
        .default_width(1000)
        .default_height(680)
        .content(&toolbar)
        .build();

    // ---- context-menu / keyboard actions (also drive the toolbar buttons) ----------------------
    let actions = gio::SimpleActionGroup::new();
    let mk = |name: &str| {
        let a = gio::SimpleAction::new(name, None);
        actions.add_action(&a);
        a
    };
    mk("open").connect_activate(clone!(
        #[strong] selection, #[strong] navigate, #[strong] reg, #[weak] window,
        move |_, _| {
            if let Some(e) = selected(&selection).into_iter().next() {
                open_entry(&reg, &navigate, &window, &e);
            }
        }
    ));
    mk("open-with").connect_activate(clone!(
        #[strong] selection, #[strong] reg, #[weak] window,
        move |_, _| {
            if let Some(e) = selected(&selection).into_iter().find(|e| !e.is_dir) {
                open_with_dialog(&reg, &window, &e);
            }
        }
    ));
    mk("copy").connect_activate(clone!(
        #[strong] selection, #[strong] clipboard, #[strong] cut_mode, #[strong] status,
        move |_, _| {
            let paths: Vec<PathBuf> = selected(&selection).into_iter().map(|e| e.path).collect();
            if !paths.is_empty() {
                let n = paths.len();
                *clipboard.borrow_mut() = paths;
                cut_mode.set(false);
                status.set_text(&format!("Copied {n} item{}", if n == 1 { "" } else { "s" }));
            }
        }
    ));
    mk("cut").connect_activate(clone!(
        #[strong] selection, #[strong] clipboard, #[strong] cut_mode, #[strong] status,
        move |_, _| {
            let paths: Vec<PathBuf> = selected(&selection).into_iter().map(|e| e.path).collect();
            if !paths.is_empty() {
                let n = paths.len();
                *clipboard.borrow_mut() = paths;
                cut_mode.set(true);
                status.set_text(&format!("Cut {n} item{}", if n == 1 { "" } else { "s" }));
            }
        }
    ));
    mk("copy-path").connect_activate(clone!(
        #[strong] selection, #[weak] window, #[strong] status,
        move |_, _| {
            let paths: Vec<String> =
                selected(&selection).iter().map(|e| e.path.to_string_lossy().to_string()).collect();
            if !paths.is_empty() {
                window.clipboard().set_text(&paths.join("\n"));
                status.set_text(if paths.len() == 1 { "Path copied" } else { "Paths copied" });
            }
        }
    ));
    mk("paste").connect_activate(clone!(
        #[strong] current, #[strong] clipboard, #[strong] cut_mode, #[strong] reload, #[strong] status, #[weak] window,
        move |_, _| {
            let dest = current.borrow().clone();
            let items = clipboard.borrow().clone();
            let moving = cut_mode.get();
            let mut ok = 0;
            for src in &items {
                let r = if moving { entry::move_into(src, &dest) } else { entry::copy_into(src, &dest) };
                if let Err(e) = r {
                    error_dialog(&window, &format!("Couldn't paste “{}”", src.display()), &e.to_string());
                    break;
                }
                ok += 1;
            }
            if moving {
                clipboard.borrow_mut().clear();
                cut_mode.set(false);
            }
            if ok > 0 {
                reload();
                let verb = if moving { "Moved" } else { "Pasted" };
                status.set_text(&format!("{verb} {ok} item{}", if ok == 1 { "" } else { "s" }));
            }
        }
    ));
    // Compress / Extract — kept as handles so the right-click menu can grey out the inapplicable one.
    let a_compress = mk("compress");
    a_compress.connect_activate(clone!(
        #[strong] selection, #[strong] current, #[strong] reload, #[strong] status, #[weak] window,
        move |_, _| {
            let items = selected(&selection);
            let dir = current.borrow().clone();
            match run_compress(&dir, &items) {
                Ok(archive) => {
                    reload();
                    let name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    status.set_text(&format!("Created {name}"));
                }
                Err(e) => error_dialog(&window, "Couldn't create archive", &e),
            }
        }
    ));
    let a_extract = mk("extract");
    a_extract.connect_activate(clone!(
        #[strong] selection, #[strong] current, #[strong] reload, #[strong] status, #[weak] window,
        move |_, _| {
            let dir = current.borrow().clone();
            let archives: Vec<Entry> = selected(&selection)
                .into_iter()
                .filter(|e| !e.is_dir && entry::is_archive(&e.name))
                .collect();
            if archives.is_empty() {
                return;
            }
            let mut ok = 0;
            let mut err = None;
            for a in &archives {
                match run_extract(&dir, a) {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        err = Some((a.name.clone(), e));
                        break;
                    }
                }
            }
            if ok > 0 {
                reload();
                status.set_text(&format!("Extracted {ok} archive{}", if ok == 1 { "" } else { "s" }));
            }
            if let Some((name, e)) = err {
                error_dialog(&window, &format!("Couldn't extract “{name}”"), &e);
            }
        }
    ));
    mk("open-terminal").connect_activate(clone!(
        #[strong] selection, #[strong] current,
        move |_, _| {
            // Open in the selected folder if one is chosen, else the current folder.
            let dir = selected(&selection)
                .into_iter()
                .find(|e| e.is_dir)
                .map(|e| e.path)
                .unwrap_or_else(|| current.borrow().clone());
            let _ = std::process::Command::new("webgen-terminal").current_dir(&dir).spawn();
        }
    ));
    mk("new-folder").connect_activate(clone!(
        #[strong] current, #[strong] reload, #[weak] window,
        move |_, _| new_folder_dialog(&window, &current, &reload)
    ));
    mk("new-file").connect_activate(clone!(
        #[strong] current, #[strong] reload, #[weak] window,
        move |_, _| new_file_dialog(&window, &current, &reload)
    ));
    mk("rename").connect_activate(clone!(
        #[strong] selection, #[strong] reload, #[weak] window,
        move |_, _| {
            let items = selected(&selection);
            match items.len() {
                1 => rename_dialog(&window, &items[0], &reload),
                n if n > 1 => error_dialog(&window, "Rename", "Select a single item to rename."),
                _ => {}
            }
        }
    ));
    mk("delete").connect_activate(clone!(
        #[strong] selection, #[strong] reload, #[weak] window,
        move |_, _| delete_selected(&window, &selected(&selection), &reload)
    ));
    mk("info").connect_activate(clone!(
        #[strong] selection, #[weak] window,
        move |_, _| {
            if let Some(e) = selected(&selection).into_iter().next() {
                info_dialog(&window, &e);
            }
        }
    ));
    window.insert_action_group("files", Some(&actions));

    // Context menu model + opener (called by each row's right-click gesture).
    let menu = gio::Menu::new();
    let sec_open = gio::Menu::new();
    sec_open.append(Some("Open"), Some("files.open"));
    sec_open.append(Some("Open With…"), Some("files.open-with"));
    sec_open.append(Some("Open Terminal Here"), Some("files.open-terminal"));
    menu.append_section(None, &sec_open);
    let sec_new = gio::Menu::new();
    sec_new.append(Some("New File"), Some("files.new-file"));
    sec_new.append(Some("New Folder"), Some("files.new-folder"));
    menu.append_section(None, &sec_new);
    let sec_clip = gio::Menu::new();
    sec_clip.append(Some("Cut"), Some("files.cut"));
    sec_clip.append(Some("Copy"), Some("files.copy"));
    sec_clip.append(Some("Paste"), Some("files.paste"));
    sec_clip.append(Some("Copy Path"), Some("files.copy-path"));
    menu.append_section(None, &sec_clip);
    let sec_arch = gio::Menu::new();
    sec_arch.append(Some("Compress"), Some("files.compress"));
    sec_arch.append(Some("Extract Here"), Some("files.extract"));
    menu.append_section(None, &sec_arch);
    let sec_meta = gio::Menu::new();
    sec_meta.append(Some("Rename"), Some("files.rename"));
    sec_meta.append(Some("Info"), Some("files.info"));
    menu.append_section(None, &sec_meta);
    let sec_del = gio::Menu::new();
    sec_del.append(Some("Delete"), Some("files.delete"));
    menu.append_section(None, &sec_del);
    *menu_opener.borrow_mut() = Some(Rc::new(clone!(
        #[strong] menu,
        #[strong] selection,
        #[strong] actions,
        #[strong] a_compress,
        #[strong] a_extract,
        move |anchor: gtk::Widget, x: f64, y: f64| {
            // Grey out Compress vs Extract: Extract is offered only when EVERY selected item is an
            // archive; Compress when there's a non-archive selection. (One of them is always dimmed.)
            let sel = selected(&selection);
            let all_archives = !sel.is_empty() && sel.iter().all(|e| !e.is_dir && entry::is_archive(&e.name));
            a_extract.set_enabled(all_archives);
            a_compress.set_enabled(!sel.is_empty() && !all_archives);
            let popover = gtk::PopoverMenu::from_model(Some(&menu));
            popover.set_parent(&anchor);
            // Defensive: also attach the "files" group to the popover so items resolve even if the
            // muxer chain from a (recycled) ListView row up to the window is ever interrupted across
            // GTK versions. (Window-level resolution works on its own here — this is belt-and-braces.)
            popover.insert_action_group("files", Some(&actions));
            popover.set_has_arrow(false);
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.connect_closed(|p| p.unparent());
            popover.popup();
        }
    )));

    // Keyboard: Ctrl+N new file, Delete / Shift+Delete delete (one confirm), F2 rename.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(clone!(
        #[weak] window,
        #[upgrade_or] glib::Propagation::Proceed,
        move |_, keyval, _code, state| {
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let act = |name: &str| {
                let _ = WidgetExt::activate_action(&window, name, None);
            };
            match keyval {
                gdk::Key::n | gdk::Key::N if ctrl => {
                    act("files.new-file");
                    glib::Propagation::Stop
                }
                gdk::Key::c | gdk::Key::C if ctrl => {
                    act("files.copy");
                    glib::Propagation::Stop
                }
                gdk::Key::x | gdk::Key::X if ctrl => {
                    act("files.cut");
                    glib::Propagation::Stop
                }
                gdk::Key::v | gdk::Key::V if ctrl => {
                    act("files.paste");
                    glib::Propagation::Stop
                }
                gdk::Key::Delete => {
                    act("files.delete");
                    glib::Propagation::Stop
                }
                gdk::Key::F2 => {
                    act("files.rename");
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        }
    ));
    window.add_controller(keys);

    // ---- wiring --------------------------------------------------------------------------------
    btn_tree.connect_toggled(clone!(
        #[weak] tree_pane,
        move |b| tree_pane.set_visible(b.is_active())
    ));

    // Open on activate (double-click / Enter): folders navigate, files open via association.
    list.connect_activate(clone!(
        #[strong] selection,
        #[strong] navigate,
        #[strong] reg,
        #[weak] window,
        move |_lv, pos| {
            if let Some(e) = item_at(&selection, pos) {
                open_entry(&reg, &navigate, &window, &e);
            }
        }
    ));

    // Address bar: type a path + Enter to go there.
    path_entry.connect_activate(clone!(
        #[strong] navigate,
        move |e| navigate(PathBuf::from(e.text().to_string()))
    ));

    // Live filter / search.
    search.connect_search_changed(clone!(
        #[strong] query,
        #[strong] reload,
        move |e| {
            *query.borrow_mut() = e.text().to_string();
            reload();
        }
    ));
    chk_recursive.connect_toggled(clone!(
        #[strong] recursive,
        #[strong] reload,
        move |c| {
            recursive.set(c.is_active());
            reload();
        }
    ));

    btn_back.connect_clicked(clone!(
        #[strong] current, #[strong] history, #[strong] forward, #[strong] reload,
        move |_| {
            if let Some(prev) = history.borrow_mut().pop() {
                let mut cur = current.borrow_mut();
                forward.borrow_mut().push(cur.clone());
                *cur = prev;
                drop(cur);
                reload();
            }
        }
    ));
    btn_forward.connect_clicked(clone!(
        #[strong] current, #[strong] history, #[strong] forward, #[strong] reload,
        move |_| {
            if let Some(next) = forward.borrow_mut().pop() {
                let mut cur = current.borrow_mut();
                history.borrow_mut().push(cur.clone());
                *cur = next;
                drop(cur);
                reload();
            }
        }
    ));
    btn_up.connect_clicked(clone!(
        #[strong] current, #[strong] navigate,
        move |_| {
            let parent = current.borrow().parent().map(Path::to_path_buf);
            if let Some(p) = parent {
                navigate(p);
            }
        }
    ));

    // Toolbar buttons drive the same actions as the context menu / shortcuts.
    btn_new.set_action_name(Some("files.new-folder"));
    btn_copy.set_action_name(Some("files.copy"));
    btn_paste.set_action_name(Some("files.paste"));
    btn_openwith.set_action_name(Some("files.open-with"));

    reload();
    window.present();
}

/// The entry at a model position in the selection.
fn item_at(selection: &gtk::MultiSelection, pos: u32) -> Option<Entry> {
    selection
        .item(pos)
        .and_downcast::<BoxedAnyObject>()
        .map(|b| b.borrow::<Entry>().clone())
}

/// All currently-selected entries.
fn selected(selection: &gtk::MultiSelection) -> Vec<Entry> {
    let bitset = selection.selection();
    let mut out = Vec::new();
    for i in 0..bitset.size() {
        let pos = bitset.nth(i as u32);
        if let Some(e) = item_at(selection, pos) {
            out.push(e);
        }
    }
    out
}

/// Open a file (or navigate into a folder). Files use the shared association, then the system
/// default; if neither works, the user is asked to choose.
fn open_entry(reg: &Reg, navigate: &Rc<dyn Fn(PathBuf)>, window: &adw::ApplicationWindow, e: &Entry) {
    if e.is_dir {
        navigate(e.path.clone());
        return;
    }
    if let (Some(r), Some(ext)) = (reg, assoc::extension_of(&e.name)) {
        if let Some(cmd) = assoc::program_for(r, &ext) {
            launch(&cmd, &e.path);
            return;
        }
    }
    let uri = gio::File::for_path(&e.path).uri();
    if gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE).is_err() {
        open_with_dialog(reg, window, e);
    }
}

/// Spawn `command <path>`, detached — the file manager doesn't wait on it.
fn launch(command: &str, path: &Path) {
    let _ = std::process::Command::new(command).arg(path).spawn();
}

/// Compress the selected items into a new `.zip` in `dir` (via Info-ZIP `zip`). Names the archive
/// after the single item, or "Archive" for several; never clobbers. Returns the archive path.
fn run_compress(dir: &Path, items: &[Entry]) -> Result<PathBuf, String> {
    if items.is_empty() {
        return Err("Nothing selected to compress.".into());
    }
    let base = if items.len() == 1 {
        Path::new(&items[0].name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| items[0].name.clone())
    } else {
        "Archive".to_string()
    };
    let mut archive = dir.join(format!("{base}.zip"));
    let mut n = 1;
    while archive.exists() {
        archive = dir.join(format!("{base} ({n}).zip"));
        n += 1;
    }
    // `zip -r -q <archive> name...` with cwd=dir, so the archive stores relative names.
    let mut cmd = std::process::Command::new("zip");
    cmd.arg("-r").arg("-q").arg(&archive).current_dir(dir);
    for e in items {
        cmd.arg(&e.name);
    }
    match cmd.status() {
        Ok(s) if s.success() => Ok(archive),
        Ok(s) => Err(format!("zip exited with {s} (is `zip` installed?)")),
        Err(e) => Err(format!("couldn't run zip: {e}")),
    }
}

/// Extract `archive` into a fresh subfolder of `dir` (via `bsdtar`, which reads zip/tar/gz/xz/7z/…).
fn run_extract(dir: &Path, archive: &Entry) -> Result<PathBuf, String> {
    // Folder name = the archive without its extension(s), e.g. "foo.tar.gz" -> "foo".
    let mut stem = Path::new(&archive.name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| archive.name.clone());
    if let Some(s) = stem.strip_suffix(".tar") {
        stem = s.to_string();
    }
    let mut out = dir.join(&stem);
    let mut n = 1;
    while out.exists() {
        out = dir.join(format!("{stem} ({n})"));
        n += 1;
    }
    std::fs::create_dir(&out).map_err(|e| e.to_string())?;
    let status = std::process::Command::new("bsdtar")
        .arg("-x")
        .arg("-f")
        .arg(&archive.path)
        .arg("-C")
        .arg(&out)
        .status();
    match status {
        Ok(s) if s.success() => Ok(out),
        Ok(s) => {
            let _ = std::fs::remove_dir_all(&out);
            Err(format!("bsdtar exited with {s}"))
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&out);
            Err(format!("couldn't run bsdtar: {e}"))
        }
    }
}

/// A small error alert. Used whenever a filesystem op fails (e.g. permission denied).
fn error_dialog(window: &adw::ApplicationWindow, heading: &str, body: &str) {
    let dialog = adw::MessageDialog::new(Some(window), Some(heading), Some(body));
    dialog.add_response("ok", "OK");
    dialog.set_default_response(Some("ok"));
    dialog.present();
}

/// Prompt for a name, then create a new folder or empty file in `dir`. `is_dir` picks which.
fn new_item_dialog(
    window: &adw::ApplicationWindow,
    dir: &Rc<RefCell<PathBuf>>,
    reload: &Rc<dyn Fn()>,
    is_dir: bool,
) {
    let (title, placeholder, kind) = if is_dir {
        ("New Folder", "Folder name", "folder")
    } else {
        ("New File", "File name", "file")
    };
    let dialog = adw::MessageDialog::new(Some(window), Some(title), None);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(placeholder));
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");
    dialog.connect_response(
        None,
        clone!(
            #[strong] dir,
            #[strong] reload,
            #[weak] window,
            move |_, resp| {
                if resp != "create" {
                    return;
                }
                let name = entry.text().to_string();
                let name = name.trim();
                if name.is_empty() {
                    return;
                }
                if name.contains('/') {
                    error_dialog(&window, title, "A name can't contain “/”.");
                    return;
                }
                let target = dir.borrow().join(name);
                if target.exists() {
                    error_dialog(&window, title, &format!("“{name}” already exists."));
                    return;
                }
                let result = if is_dir {
                    std::fs::create_dir(&target)
                } else {
                    std::fs::File::create(&target).map(|_| ())
                };
                match result {
                    Ok(()) => reload(),
                    Err(e) => error_dialog(&window, &format!("Couldn't create {kind}"), &e.to_string()),
                }
            }
        ),
    );
    dialog.present();
}

fn new_folder_dialog(window: &adw::ApplicationWindow, dir: &Rc<RefCell<PathBuf>>, reload: &Rc<dyn Fn()>) {
    new_item_dialog(window, dir, reload, true);
}

fn new_file_dialog(window: &adw::ApplicationWindow, dir: &Rc<RefCell<PathBuf>>, reload: &Rc<dyn Fn()>) {
    new_item_dialog(window, dir, reload, false);
}

/// Rename a single item.
fn rename_dialog(window: &adw::ApplicationWindow, item: &Entry, reload: &Rc<dyn Fn()>) {
    let dialog = adw::MessageDialog::new(Some(window), Some("Rename"), None);
    let field = gtk::Entry::new();
    field.set_text(&item.name);
    field.set_activates_default(true);
    dialog.set_extra_child(Some(&field));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("rename"));
    dialog.set_close_response("cancel");
    let old_path = item.path.clone();
    let old_name = item.name.clone();
    dialog.connect_response(
        None,
        clone!(
            #[strong] reload,
            #[weak] window,
            move |_, resp| {
                if resp != "rename" {
                    return;
                }
                let new_name = field.text().to_string();
                let new_name = new_name.trim();
                if new_name.is_empty() || new_name == old_name {
                    return;
                }
                if new_name.contains('/') {
                    error_dialog(&window, "Rename", "A name can't contain “/”.");
                    return;
                }
                let new_path = old_path.parent().unwrap_or(Path::new(".")).join(new_name);
                if new_path.exists() {
                    error_dialog(&window, "Rename", &format!("“{new_name}” already exists."));
                    return;
                }
                match std::fs::rename(&old_path, &new_path) {
                    Ok(()) => reload(),
                    Err(e) => error_dialog(&window, "Couldn't rename", &e.to_string()),
                }
            }
        ),
    );
    dialog.present();
}

/// Delete the selected items — always one confirmation, permanent + recursive, bailing with a
/// sensible message on the first failure (e.g. permission denied). No per-item statting.
fn delete_selected(window: &adw::ApplicationWindow, items: &[Entry], reload: &Rc<dyn Fn()>) {
    if items.is_empty() {
        return;
    }
    let body = if items.len() == 1 {
        format!("Permanently delete “{}”? This cannot be undone.", items[0].name)
    } else {
        format!("Permanently delete these {} items? This cannot be undone.", items.len())
    };
    let dialog = adw::MessageDialog::new(Some(window), Some("Delete"), Some(&body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let paths: Vec<PathBuf> = items.iter().map(|e| e.path.clone()).collect();
    dialog.connect_response(
        None,
        clone!(
            #[strong] reload,
            #[weak] window,
            move |_, resp| {
                if resp != "delete" {
                    return;
                }
                for p in &paths {
                    if let Err(e) = entry::remove_path(p) {
                        error_dialog(&window, &format!("Couldn't delete “{}”", p.display()), &e.to_string());
                        break; // bail on the first failure
                    }
                }
                reload();
            }
        ),
    );
    dialog.present();
}

/// Show file/folder info: type, size, permissions (octal + rwx), owner, group.
fn info_dialog(window: &adw::ApplicationWindow, item: &Entry) {
    let info = match meta::gather(&item.path) {
        Ok(i) => i,
        Err(e) => {
            error_dialog(window, "Couldn't read item", &e.to_string());
            return;
        }
    };
    let dialog = adw::MessageDialog::new(Some(window), Some(&info.name), None);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    let add = |title: &str, value: &str| {
        let row = adw::ActionRow::new();
        row.set_title(title);
        row.set_subtitle(value);
        list.append(&row);
    };
    add("Location", &info.location);
    add("Type", &info.kind);
    add("Size", &info.size);
    add("Permissions", &format!("{}  ({})", info.perms_octal, info.perms_rwx));
    add("Owner", &info.owner);
    add("Group", &info.group);
    add("Modified", &info.modified);
    add("Accessed", &info.accessed);
    add("Created", &info.created);
    dialog.set_extra_child(Some(&list));
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.present();
}

/// Choose a program to open a file with, optionally remembering it for the extension.
fn open_with_dialog(reg: &Reg, window: &adw::ApplicationWindow, e: &Entry) {
    let win = gtk::Window::builder()
        .title("Open With")
        .transient_for(window)
        .modal(true)
        .default_width(360)
        .build();

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);

    let heading = gtk::Label::new(None);
    heading.set_xalign(0.0);
    heading.set_markup(&format!("Open <b>{}</b> with:", glib::markup_escape_text(&e.name)));
    content.append(&heading);

    let listbox = gtk::ListBox::new();
    listbox.set_selection_mode(gtk::SelectionMode::Single);
    listbox.add_css_class("boxed-list");
    for p in assoc::PROGRAMS {
        let row = adw::ActionRow::new();
        row.set_title(p.name);
        row.set_subtitle(p.command);
        let img = gtk::Image::from_icon_name(p.icon);
        img.set_pixel_size(24);
        row.add_prefix(&img);
        listbox.append(&row);
    }
    listbox.select_row(listbox.row_at_index(0).as_ref());
    content.append(&listbox);

    let ext = assoc::extension_of(&e.name);
    let remember = gtk::CheckButton::with_label(&match &ext {
        Some(x) => format!("Always open .{x} files this way"),
        None => "Remember for this file type".to_string(),
    });
    remember.set_sensitive(ext.is_some());
    content.append(&remember);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let open = gtk::Button::with_label("Open");
    open.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&open);
    content.append(&buttons);

    outer.append(&content);
    win.set_child(Some(&outer));

    cancel.connect_clicked(clone!(#[weak] win, move |_| win.close()));
    let path = e.path.clone();
    open.connect_clicked(clone!(
        #[weak] win,
        #[strong] reg,
        #[strong] remember,
        #[strong] listbox,
        move |_| {
            let idx = listbox.selected_row().map(|r| r.index()).unwrap_or(0).max(0) as usize;
            if let Some(p) = assoc::PROGRAMS.get(idx) {
                launch(p.command, &path);
                if remember.is_active() {
                    if let (Some(r), Some(x)) = (&reg, &ext) {
                        assoc::set_program(r, x, p.command);
                    }
                }
            }
            win.close();
        }
    ));

    win.present();
}
