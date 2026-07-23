//! The left-hand collapsible **folder tree**: a lazy `TreeListModel` of directories, rendered with
//! `TreeExpander`. Selecting a folder navigates the file list on the right.

use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// Subdirectories of `dir` (folders only), as a `ListStore<gio::File>`, or `None` if there are
/// none (so the row shows no expander).
fn children_of(dir: &Path, show_hidden: bool) -> Option<gio::ListModel> {
    let file = gio::File::for_path(dir);
    let enumerator = file
        .enumerate_children(
            "standard::name,standard::type,standard::is-hidden",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let store = gio::ListStore::new::<gio::File>();
    let mut names: Vec<(String, gio::File)> = Vec::new();
    while let Ok(Some(info)) = enumerator.next_file(gio::Cancellable::NONE) {
        if info.file_type() != gio::FileType::Directory {
            continue;
        }
        if info.is_hidden() && !show_hidden {
            continue;
        }
        names.push((info.name().to_string_lossy().to_lowercase(), file.child(info.name())));
    }
    if names.is_empty() {
        return None;
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, child) in names {
        store.append(&child);
    }
    Some(store.upcast())
}

/// Build the folder-tree widget rooted at Home and the filesystem root. `on_navigate` fires with the
/// selected folder's path.
pub fn build(show_hidden: bool, on_navigate: impl Fn(PathBuf) + 'static) -> gtk::ScrolledWindow {
    let root = gio::ListStore::new::<gio::File>();
    root.append(&gio::File::for_path(glib::home_dir()));
    root.append(&gio::File::for_path("/"));

    let tree = gtk::TreeListModel::new(root, false, false, move |obj| {
        let file = obj.downcast_ref::<gio::File>()?;
        let path = file.path()?;
        children_of(&path, show_hidden)
    });

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name("folder");
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&icon);
        row.append(&label);
        let expander = gtk::TreeExpander::new();
        expander.set_child(Some(&row));
        item.set_child(Some(&expander));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
        let expander = item.child().and_downcast::<gtk::TreeExpander>().unwrap();
        expander.set_list_row(Some(&row));
        let Some(file) = row.item().and_downcast::<gio::File>() else { return };
        let hbox = expander.child().and_downcast::<gtk::Box>().unwrap();
        let label = hbox.last_child().and_downcast::<gtk::Label>().unwrap();
        let name = file
            .basename()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|s| s != "/")
            .unwrap_or_else(|| "Computer".to_string());
        label.set_text(&name);
    });

    let selection = gtk::SingleSelection::new(Some(tree));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    selection.set_selected(gtk::INVALID_LIST_POSITION);
    selection.connect_selected_item_notify(move |sel| {
        if let Some(row) = sel.selected_item().and_downcast::<gtk::TreeListRow>() {
            if let Some(file) = row.item().and_downcast::<gio::File>() {
                if let Some(path) = file.path() {
                    on_navigate(path);
                }
            }
        }
    });

    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("navigation-sidebar");

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&list));
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_width_request(220);
    scroller
}
