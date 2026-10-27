//! Navigation permanente et accueil, sans modifier les étapes de la compétition.
use crate::{app::App, visuals};
use bp_core::{i18n::tr, stage::StageKind};
use bp_gtk::glade::Glade;
use gtk4::{self as gtk, prelude::*};
use std::{
    cell::Cell,
    rc::{Rc, Weak},
};

pub struct Shell {
    app: Weak<App>,
    stack: gtk::Stack,
    navigation: Vec<(gtk::Button, Vec<StageKind>)>,
    home: gtk::Button,
    organisation: gtk::Button,
    /// Puce rouge : signalements des arbitres en cours.
    organisation_dot: gtk::Label,
    preferences: gtk::Button,
    preferences_shortcut: gtk::Button,
    publish: gtk::Button,
    resume: gtk::Button,
    /// Vignettes des compétitions récentes (accueil).
    recent: gtk::FlowBox,
    recent_title: gtk::Label,
    at_home: Cell<bool>,
}

impl Shell {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let layout: gtk::Box = app.glade.get("vbox1");
        let workspace: gtk::Widget = app.glade.get("hbox1");
        Glade::detach_from_parent(&workspace);
        if let Some(logo) = app.glade.widget("logo") {
            Glade::detach_from_parent(&logo);
        }
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.add_named(&workspace, Some("workspace"));

        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 5);
        sidebar.add_css_class("bp-sidebar");
        // Largeur imposée par le séparateur, jamais par le contenu.
        sidebar.set_hexpand(false);
        sidebar.set_width_request(150);
        let caption = gtk::Label::new(Some(&tr("TOURNAMENT")));
        caption.set_xalign(0.0);
        caption.add_css_class("bp-nav-caption");
        sidebar.append(&caption);
        let home = visuals::action("home", "Dashboard");
        home.add_css_class("bp-nav-item");
        sidebar.append(&home);
        let mut navigation = Vec::new();
        for (icon, title, kinds) in [
            ("participants", "Participants", vec![StageKind::Checkin]),
            (
                "pools",
                "Pools",
                vec![
                    StageKind::PoolSupervisor,
                    StageKind::PoolAllocator,
                    StageKind::YouthTraining,
                    StageKind::DivisionRound,
                    StageKind::Lobby,
                ],
            ),
            ("bracket", "Table", vec![StageKind::TableSupervisor]),
            (
                "rank",
                "General classification",
                vec![StageKind::GeneralClassification],
            ),
        ] {
            let button = visuals::action(icon, title);
            button.add_css_class("bp-nav-item");
            let weak = Rc::downgrade(app);
            let targets = kinds.clone();
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.navigate_to(&targets);
                }
            });
            sidebar.append(&button);
            navigation.push((button, kinds));
        }
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_margin_top(14);
        separator.set_margin_bottom(10);
        sidebar.append(&separator);
        let caption = gtk::Label::new(Some(&tr("MANAGEMENT")));
        caption.set_xalign(0.0);
        caption.add_css_class("bp-nav-caption");
        sidebar.append(&caption);
        let organisation = visuals::action("swords", "Organisation");
        organisation.add_css_class("bp-nav-item");
        let weak = Rc::downgrade(app);
        organisation.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.open_management();
            }
        });
        sidebar.append(&organisation);
        let organisation_dot = gtk::Label::new(None);
        organisation_dot.add_css_class("bp-nav-dot");
        organisation_dot.set_visible(false);
        organisation_dot.set_hexpand(true);
        organisation_dot.set_halign(gtk::Align::End);
        if let Some(content) = organisation.child().and_downcast::<gtk::Box>() {
            content.append(&organisation_dot);
        }
        let preferences = visuals::action("settings", "Preferences");
        preferences.add_css_class("bp-nav-item");
        let weak = Rc::downgrade(app);
        preferences.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.open_contest_preferences();
            }
        });
        sidebar.append(&preferences);
        let season = visuals::action("season", "Season ranking");
        season.add_css_class("bp-nav-item");
        let weak = Rc::downgrade(app);
        season.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::season_window::open(&app);
            }
        });
        sidebar.append(&season);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        sidebar.append(&spacer);
        let art = visuals::background_art("bp-sidebar-art", 90);
        art.set_opacity(0.48);
        art.set_margin_top(12);
        sidebar.append(&art);
        let brand = visuals::brand_header(118, 36);
        brand.set_halign(gtk::Align::Center);
        brand.set_margin_top(6);
        sidebar.append(&brand);
        let version = gtk::Label::new(Some(&format!("Rust · {}", bp_core::contest::VERSION)));
        version.add_css_class("bp-muted");
        sidebar.append(&version);
        // Barre latérale redimensionnable (largeur mémorisée).
        let main = gtk::Paned::new(gtk::Orientation::Horizontal);
        main.set_vexpand(true);
        main.set_wide_handle(false);
        main.set_start_child(Some(&sidebar));
        main.set_resize_start_child(false);
        main.set_shrink_start_child(false);
        let (welcome, resume, recent, recent_title) = Self::welcome(app);
        stack.add_named(&welcome, Some("welcome"));
        main.set_end_child(Some(&stack));
        main.set_resize_end_child(true);
        main.set_shrink_end_child(false);
        let saved = app.tournament.borrow().config.get_integer("Tournament", "sidebar_width");
        main.set_position(if saved >= 150 { saved as i32 } else { 210 });
        let weak = Rc::downgrade(app);
        main.connect_position_notify(move |p| {
            if let Some(app) = weak.upgrade() {
                if let Ok(mut t) = app.tournament.try_borrow_mut() {
                    t.config.set_integer("Tournament", "sidebar_width", p.position() as i64);
                }
            }
        });
        layout.append(&main);

        let brandbar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        brandbar.add_css_class("bp-brandbar");
        let toggle = gtk::Button::from_icon_name("sidebar-show-symbolic");
        toggle.set_tooltip_text(Some(&tr("Show or hide navigation")));
        let weak_sidebar = sidebar.downgrade();
        toggle.connect_clicked(move |_| {
            if let Some(sidebar) = weak_sidebar.upgrade() {
                sidebar.set_visible(!sidebar.is_visible());
            }
        });
        brandbar.append(&toggle);
        brandbar.append(&visuals::brand_header(131, 40));
        let subtitle = gtk::Label::new(Some(&tr("Fencing tournament")));
        subtitle.add_css_class("bp-muted");
        subtitle.set_hexpand(true);
        subtitle.set_xalign(0.0);
        brandbar.append(&subtitle);
        let help = visuals::action("help", "Help");
        let weak = Rc::downgrade(app);
        help.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.on_user_manual();
            }
        });
        brandbar.append(&help);
        let preferences_shortcut = visuals::action("settings", "Preferences");
        let weak = Rc::downgrade(app);
        preferences_shortcut.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.open_contest_preferences();
            }
        });
        brandbar.append(&preferences_shortcut);
        // Publication sur ManageComp (compete.managecomp.org).
        let publish = visuals::action("publish", "Publish");
        publish.set_tooltip_text(Some(&tr("Public results page (compete.managecomp.org)")));
        let weak = Rc::downgrade(app);
        publish.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::publish::show_dialog(&app);
            }
        });
        brandbar.append(&publish);
        layout.prepend(&brandbar);

        let shell = Rc::new(Self {
            app: Rc::downgrade(app),
            stack,
            navigation,
            home,
            organisation,
            organisation_dot,
            preferences,
            preferences_shortcut,
            publish,
            resume,
            recent,
            recent_title,
            at_home: Cell::new(true),
        });
        let weak = Rc::downgrade(&shell);
        shell.home.connect_clicked(move |_| {
            if let Some(shell) = weak.upgrade() {
                shell.show_home();
            }
        });
        let weak = Rc::downgrade(&shell);
        shell.resume.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade().and_then(|shell| shell.app.upgrade()) {
                app.resume_contest();
            }
        });
        let weak = Rc::downgrade(&shell);
        app.notebook.connect_switch_page(move |_, _, _| {
            let weak = weak.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(shell) = weak.upgrade() {
                    shell.refresh_navigation();
                }
            });
        });
        shell.refresh();
        shell
    }

    fn welcome(app: &Rc<App>) -> (gtk::Widget, gtk::Button, gtk::FlowBox, gtk::Label) {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 24);
        page.add_css_class("bp-welcome");
        page.set_valign(gtk::Align::Center);
        page.set_halign(gtk::Align::Center);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_hexpand(true);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&page));
        let art = visuals::background_art("bp-hero-art", 180);
        art.set_width_request(480);
        page.append(&art);
        let title = gtk::Label::new(Some(&tr("Your tournament starts here")));
        title.add_css_class("bp-welcome-title");
        page.append(&title);
        let description = gtk::Label::new(Some(&tr("Open a competition or create a new event.\nParticipants, pools and organisation, all in one place.")));
        description.set_justify(gtk::Justification::Center);
        description.add_css_class("bp-muted");
        page.append(&description);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        actions.set_halign(gtk::Align::Center);
        let new = gtk::Button::with_label(&tr("New competition"));
        new.add_css_class("suggested-action");
        let weak = Rc::downgrade(app);
        new.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.on_new();
            }
        });
        actions.append(&new);
        let youth = gtk::Button::with_label(&tr("New youth training"));
        youth.add_css_class("suggested-action");
        let weak = Rc::downgrade(app);
        youth.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.on_new_youth();
            }
        });
        actions.append(&youth);
        let open = gtk::Button::with_label(&tr("Open a competition"));
        let weak = Rc::downgrade(app);
        open.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.on_open();
            }
        });
        actions.append(&open);
        let season = gtk::Button::with_label(&tr("Season ranking"));
        let weak = Rc::downgrade(app);
        season.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                crate::season_window::open(&app);
            }
        });
        actions.append(&season);
        page.append(&actions);
        let resume = gtk::Button::with_label(&tr("Resume competition"));
        resume.set_halign(gtk::Align::Center);
        page.append(&resume);

        // Compétitions récentes : vignettes cliquables.
        let recent_title = gtk::Label::new(Some(&tr("Recent competitions")));
        recent_title.add_css_class("bp-recent-title");
        recent_title.set_xalign(0.0);
        recent_title.set_margin_top(8);
        page.append(&recent_title);
        let recent = gtk::FlowBox::new();
        recent.set_selection_mode(gtk::SelectionMode::None);
        recent.set_homogeneous(true);
        recent.set_max_children_per_line(4);
        recent.set_min_children_per_line(1);
        recent.set_column_spacing(12);
        recent.set_row_spacing(12);
        recent.set_halign(gtk::Align::Center);
        page.append(&recent);
        (scroller.upcast(), resume, recent, recent_title)
    }

    /// Remplit les vignettes des fichiers récents (8 au plus).
    fn refresh_recent(&self) {
        let Some(app) = self.app.upgrade() else { return };
        while let Some(child) = self.recent.first_child() {
            self.recent.remove(&child);
        }
        let files: Vec<std::path::PathBuf> = app.tournament.borrow().recent_files().into_iter().take(8).collect();
        self.recent_title.set_visible(!files.is_empty());
        for path in files {
            let card = recent_card(&path);
            let weak = self.app.clone();
            let opened = path.clone();
            card.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.open_recent(&opened);
                }
            });
            // Retrait de l'accueil : le fichier n'est pas effacé.
            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.add_css_class("flat");
            remove.add_css_class("circular");
            remove.add_css_class("bp-recent-remove");
            remove.set_halign(gtk::Align::End);
            remove.set_valign(gtk::Align::Start);
            remove.set_tooltip_text(Some(&tr("Remove from recent competitions (the file is kept)")));
            let weak = self.app.clone();
            remove.connect_clicked(move |_| {
                let Some(app) = weak.upgrade() else { return };
                let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let question = format!("{} « {} » ?", tr("Remove from recent competitions"), gtk::glib::markup_escape_text(&name));
                if bp_gtk::dialogs::confirm(Some(&app.window), &question, Some(&tr("The file is not deleted.")), None) {
                    app.tournament.borrow_mut().remove_recent(&path);
                    if let Some(shell) = app.shell.borrow().as_ref() {
                        shell.refresh_recent();
                    }
                }
            });
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&card));
            overlay.add_overlay(&remove);
            self.recent.insert(&overlay, -1);
        }
    }

    /// Signalements des arbitres en cours (absence d'un tireur).
    pub fn set_reports(&self, open: usize) {
        self.organisation_dot.set_visible(open > 0);
        self.organisation_dot.set_text(&open.to_string());
        self.organisation_dot.set_tooltip_text(Some(&format!("{open} {}", tr("referee report(s): see Referees on phones"))));
    }

    pub fn show_home(&self) {
        self.at_home.set(true);
        self.refresh_recent();
        self.stack.set_visible_child_name("welcome");
        self.refresh_navigation();
    }

    pub fn refresh(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let has_pages = app.notebook.n_pages() != 0;
        app.notebook.set_show_tabs(app.notebook.n_pages() > 1);
        self.at_home.set(!has_pages);
        if !has_pages {
            self.refresh_recent();
        }
        self.stack
            .set_visible_child_name(if has_pages { "workspace" } else { "welcome" });
        self.refresh_navigation();
    }

    pub fn refresh_navigation(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let page = app.active_contest();
        let schedule = page
            .as_ref()
            .and_then(|page| page.schedule.borrow().clone());
        let organisation = app.management.borrow().as_ref().is_some_and(|m| {
            app.notebook.page_num(m.widget()).is_some()
                && app.notebook.page_num(m.widget()) == app.notebook.current_page()
        });
        for (button, kinds) in &self.navigation {
            button.set_sensitive(
                schedule
                    .as_ref()
                    .is_some_and(|s| s.has_visible_stage(kinds)),
            );
            let selected = !self.at_home.get()
                && !organisation
                && schedule
                    .as_ref()
                    .and_then(|s| s.visible_stage_kind())
                    .is_some_and(|kind| kinds.contains(&kind));
            set_selected(button, selected);
        }
        self.preferences.set_sensitive(page.is_some());
        self.preferences_shortcut.set_sensitive(page.is_some());
        self.publish.set_sensitive(page.is_some());
        self.resume.set_visible(page.is_some());
        set_selected(&self.home, self.at_home.get());
        set_selected(&self.organisation, organisation && !self.at_home.get());
    }
}

/// En-tête d'un fichier de compétition, lu sans charger tout le fichier :
/// titre, arme, sexe, catégorie, date et couleur de l'élément racine.
struct RecentSummary {
    title: String,
    details: String,
    date: String,
    color: Option<String>,
}

fn recent_summary(path: &std::path::Path) -> Option<RecentSummary> {
    use std::io::Read;
    let mut head = vec![0u8; 16 * 1024];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    let text = String::from_utf8_lossy(&head[..n]).into_owned();
    let start = text.find("<Competition")?;
    let end = text[start..].find('>').map(|e| start + e)?;
    let tag = &text[start..end];
    let attr = |name: &str| -> Option<String> {
        let key = format!(" {name}=\"");
        let i = tag.find(&key)? + key.len();
        let j = tag[i..].find('"')? + i;
        let v = tag[i..j].replace("&apos;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
        Some(v).filter(|v| !v.trim().is_empty())
    };
    let weapon = attr("Arme").and_then(|w| bp_core::weapon::Weapon::from_xml(&w)).map(|w| tr(w.image));
    let gender = attr("Sexe").and_then(|g| {
        bp_core::contest::GENDER_XML_IMAGES
            .iter()
            .position(|x| *x == g)
            .or_else(|| bp_core::contest::GENDER_XML_ALIASES.iter().position(|x| *x == g))
            .map(|i| tr(bp_core::contest::GENDER_IMAGES[i]))
    });
    let details = [weapon, gender, attr("Categorie")].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let title = attr("TitreLong").unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    let date = attr("Date").unwrap_or_default().replace('.', "/");
    Some(RecentSummary { title, details, date, color: attr("Couleur") })
}

/// Vignette d'une compétition récente.
fn recent_card(path: &std::path::Path) -> gtk::Button {
    let summary = recent_summary(path);
    let card = gtk::Button::new();
    card.add_css_class("bp-recent-card");
    card.set_width_request(210);
    card.set_tooltip_text(Some(&path.to_string_lossy()));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let stripe = gtk::Box::new(gtk::Orientation::Vertical, 0);
    stripe.set_width_request(5);
    stripe.add_css_class("bp-recent-stripe");
    if let Some(color) = summary.as_ref().and_then(|s| s.color.clone()) {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(&format!(".bp-recent-stripe {{ background-color: {color}; }}"));
        #[allow(deprecated)]
        stripe.style_context().add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    row.append(&stripe);
    let copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
    copy.set_hexpand(true);
    let file = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let (title, details, date) = match &summary {
        Some(s) => (s.title.clone(), s.details.clone(), s.date.clone()),
        None => (file.clone(), String::new(), String::new()),
    };
    let t = gtk::Label::new(Some(&title));
    t.add_css_class("bp-recent-name");
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.set_max_width_chars(24);
    copy.append(&t);
    if !details.is_empty() {
        let d = gtk::Label::new(Some(&details));
        d.add_css_class("bp-recent-details");
        d.set_xalign(0.0);
        d.set_ellipsize(gtk::pango::EllipsizeMode::End);
        copy.append(&d);
    }
    let meta = [date, file].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let m = gtk::Label::new(Some(&meta));
    m.add_css_class("bp-muted");
    m.set_xalign(0.0);
    m.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    m.set_max_width_chars(28);
    copy.append(&m);
    row.append(&copy);
    card.set_child(Some(&row));
    card
}

fn set_selected(button: &gtk::Button, selected: bool) {
    if selected {
        button.add_css_class("bp-nav-active");
    } else {
        button.remove_css_class("bp-nav-active");
    }
}
