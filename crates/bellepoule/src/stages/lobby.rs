//! Lobby (série) : réglages (formule), saisie des arrivées lobby par lobby
//! et manche par manche (place, même place pour des ex aequo, « A » pour un
//! abandon), points et classements.

use bp_core::{
    i18n::tr,
    player::{PlayerRef, PlayerStore},
    rounds::lobby::{choice_label, lobby_choices, Distribution, Lobby, LobbySettings, Standing, TeamMode, TeamRule, ABANDON},
    stage::Stage,
};
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use super::StageScreen;
use crate::contest_page::ContestPage;

pub struct LobbyScreen {
    page: Weak<ContestPage>,
    index: usize,
    root: gtk::Box,
    configuration: gtk::Widget,
    summary: gtk::Label,
    lobby_choice: gtk::DropDown,
    lobby_count: Cell<usize>,
    show_ranking: gtk::ToggleButton,
    content: gtk::ScrolledWindow,
    // Formule (nombre de lobbys) et répartition, comme pour les poules.
    formula: gtk::DropDown,
    formula_values: RefCell<Vec<u32>>,
    distribution: gtk::DropDown,
    // Réglages
    races: gtk::SpinButton,
    scale: gtk::Entry,
    abandon: gtk::SpinButton,
    qualified: gtk::SpinButton,
    per_lobby_mode: gtk::DropDown,
    per_lobby: gtk::SpinButton,
    swapping: Vec<(String, gtk::CheckButton)>,
    team_box: gtk::Box,
    team_mode: gtk::DropDown,
    team_rule: gtk::DropDown,
    team_best: gtk::SpinButton,
    updating: Cell<bool>,
    me: RefCell<Weak<LobbyScreen>>,
}

fn spin(min: f64, max: f64) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, 1.0);
    s.set_digits(0);
    bp_gtk::util::scroll_passes_through(&s);
    s
}

fn dropdown(items: &[String]) -> gtk::DropDown {
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let d = gtk::DropDown::from_strings(&refs);
    bp_gtk::util::scroll_passes_through(&d);
    d
}

fn labelled(text: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_size_request(240, -1);
    line.append(&label);
    line.append(widget);
    line
}

fn name_of(players: &PlayerStore, r: PlayerRef) -> (String, String) {
    players
        .get(r)
        .map(|p| (format!("{} {}", p.name(), p.first_name()).trim().to_string(), p.str_attr("club").unwrap_or_default().to_string()))
        .unwrap_or_default()
}

/// « 3 », « A » (abandon) ou vide.
fn finish_text(f: Option<i32>) -> String {
    match f {
        Some(ABANDON) => "A".into(),
        Some(p) => p.to_string(),
        None => String::new(),
    }
}

/// Saisie : nombre (place), « A » (abandon), vide (effacer).
fn parse_finish(text: &str) -> Option<Option<i32>> {
    let t = text.trim().to_uppercase();
    if t.is_empty() {
        return Some(None);
    }
    if t.starts_with('A') {
        return Some(Some(ABANDON));
    }
    t.parse::<i32>().ok().filter(|p| *p > 0).map(Some)
}

/// Données d'affichage lues dans la compétition.
struct View {
    locked: bool,
    /// Participants qui courent (formules possibles).
    competitors: usize,
    settings: LobbySettings,
    team_event: bool,
    nb_qualified: u32,
    lobbies: Vec<LobbyView>,
    /// Classement de la phase : (place, nom, club, bilan).
    ranking: Vec<(u32, String, String, Standing)>,
    /// Épreuve par équipes, tireurs qui courent : classement individuel.
    individual: Vec<(u32, String, String, Standing)>,
    has_results: bool,
}

struct LobbyView {
    rows: Vec<RowView>,
    done: usize,
    /// Organisation : « Piste 03 @ 14:20 · Arbitre : … ».
    roadmap: String,
}

struct RowView {
    player: PlayerRef,
    name: String,
    club: String,
    finishes: Vec<Option<i32>>,
    standing: Standing,
    place: u32,
}

impl LobbyScreen {
    pub fn new(page: &Rc<ContestPage>, index: usize) -> Rc<Self> {
        // Réglages (préférences de la phase, dans la formule).
        let config = gtk::Box::new(gtk::Orientation::Vertical, 6);
        config.set_margin_top(10);
        config.set_margin_start(10);
        config.set_margin_end(10);
        config.set_margin_bottom(10);
        let races = spin(1.0, 50.0);
        let scale = gtk::Entry::new();
        scale.set_placeholder_text(Some(&tr("empty: N points for the 1st, 1 for the last")));
        scale.set_tooltip_text(Some(&tr("Points for the 1st, 2nd, 3rd… separated by commas (e.g. 25, 18, 15, 12, 10)")));
        scale.set_hexpand(true);
        let abandon = spin(0.0, 1000.0);
        let qualified = spin(0.0, 10000.0);
        qualified.set_tooltip_text(Some(&tr("0: everybody")));
        let per_lobby_mode = dropdown(&[tr("in total (merged ranking)"), tr("per lobby")]);
        let per_lobby = spin(1.0, 1000.0);
        config.append(&labelled(&tr("Races"), &races));
        config.append(&labelled(&tr("Points per place"), &scale));
        config.append(&labelled(&tr("Points for an abandon"), &abandon));
        let q = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        q.append(&per_lobby_mode);
        q.append(&per_lobby);
        config.append(&labelled(&tr("Qualified"), &q));
        config.append(&labelled(&tr("Qualified in total"), &qualified));
        let swap_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let mut swapping = Vec::new();
        for (code, label) in [("club", tr("club")), ("league", tr("league")), ("country", tr("country"))] {
            let check = gtk::CheckButton::with_label(&label);
            swap_box.append(&check);
            swapping.push((code.to_string(), check));
        }
        config.append(&labelled(&tr("Separate in the lobbies by"), &swap_box));
        let team_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let team_mode = dropdown(&[tr("the team races as one competitor"), tr("each fencer of the team races")]);
        let team_rule = dropdown(&[tr("sum of the points of its fencers"), tr("sum of its best fencers")]);
        let team_best = spin(1.0, 20.0);
        team_box.append(&labelled(&tr("Team event"), &team_mode));
        let rule = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        rule.append(&team_rule);
        rule.append(&team_best);
        team_box.append(&labelled(&tr("Team ranking"), &rule));
        config.append(&team_box);
        let help = gtk::Label::new(Some(&tr("Everybody in a lobby races at the same time; each race gives points by place of arrival. Ties: same place, same points. Ranking: points, then wins, then best place.")));
        help.set_wrap(true);
        help.set_xalign(0.0);
        help.add_css_class("bp-muted");
        config.append(&help);
        let frame = gtk::Frame::new(None);
        frame.set_child(Some(&config));

        // Écran de la phase.
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.add_css_class("toolbar");
        bar.set_margin_start(8);
        bar.set_margin_top(4);
        let lobby_choice = gtk::DropDown::from_strings(&["Lobby 1"]);
        bp_gtk::util::scroll_passes_through(&lobby_choice);
        // Formule et répartition : comme la composition des poules.
        bar.append(&gtk::Label::new(Some(&tr("Formula"))));
        let formula = gtk::DropDown::from_strings(&["1 lobby"]);
        bp_gtk::util::scroll_passes_through(&formula);
        bar.append(&formula);
        let distribution = dropdown(&[tr("balanced (serpentine)"), tr("by equivalent strength")]);
        distribution.set_tooltip_text(Some(&tr("Balanced: serpentine from the entry ranking, like pools. By equivalent strength: the best together in lobby 1, the next ones in lobby 2…")));
        bar.append(&distribution);
        bar.append(&lobby_choice);
        let summary = gtk::Label::new(None);
        summary.set_hexpand(true);
        summary.set_xalign(0.0);
        summary.add_css_class("bp-muted");
        bar.append(&summary);
        let show_ranking = gtk::ToggleButton::with_label(&tr("Ranking"));
        bar.append(&show_ranking);
        let print = gtk::Button::from_icon_name("document-print-symbolic");
        print.set_tooltip_text(Some(&tr("Print")));
        bar.append(&print);
        root.append(&bar);
        let hint = gtk::Label::new(Some(&tr("Type the place of each fencer in the race (the same place for ties, A for an abandon), then Enter.")));
        hint.set_xalign(0.0);
        hint.set_margin_start(10);
        hint.add_css_class("bp-muted");
        root.append(&hint);
        let content = gtk::ScrolledWindow::new();
        content.set_vexpand(true);
        root.append(&content);

        let screen = Rc::new(Self {
            page: Rc::downgrade(page),
            index,
            root,
            configuration: frame.upcast(),
            summary,
            lobby_choice,
            lobby_count: Cell::new(1),
            show_ranking,
            content,
            formula,
            formula_values: RefCell::new(Vec::new()),
            distribution,
            races,
            scale,
            abandon,
            qualified,
            per_lobby_mode,
            per_lobby,
            swapping,
            team_box,
            team_mode,
            team_rule,
            team_best,
            updating: Cell::new(false),
            me: RefCell::new(Weak::new()),
        });
        *screen.me.borrow_mut() = Rc::downgrade(&screen);
        for d in [&screen.formula, &screen.distribution] {
            let w = Rc::downgrade(&screen);
            d.connect_selected_notify(move |_| {
                if let Some(s) = w.upgrade() {
                    s.on_composition_changed();
                }
            });
        }
        for s in [&screen.races, &screen.abandon, &screen.qualified, &screen.per_lobby, &screen.team_best] {
            let w = Rc::downgrade(&screen);
            s.connect_value_changed(move |_| {
                if let Some(s) = w.upgrade() {
                    s.on_settings_changed();
                }
            });
        }
        for d in [&screen.per_lobby_mode, &screen.team_mode, &screen.team_rule] {
            let w = Rc::downgrade(&screen);
            d.connect_selected_notify(move |_| {
                if let Some(s) = w.upgrade() {
                    s.on_settings_changed();
                }
            });
        }
        for (_, c) in &screen.swapping {
            let w = Rc::downgrade(&screen);
            c.connect_toggled(move |_| {
                if let Some(s) = w.upgrade() {
                    s.on_settings_changed();
                }
            });
        }
        let w = Rc::downgrade(&screen);
        screen.scale.connect_activate(move |_| {
            if let Some(s) = w.upgrade() {
                s.on_settings_changed();
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let w = Rc::downgrade(&screen);
        focus.connect_leave(move |_| {
            if let Some(s) = w.upgrade() {
                s.on_settings_changed();
            }
        });
        screen.scale.add_controller(focus);
        let w = Rc::downgrade(&screen);
        screen.lobby_choice.connect_selected_notify(move |_| {
            if let Some(s) = w.upgrade() {
                if !s.updating.get() {
                    s.refresh();
                }
            }
        });
        let w = Rc::downgrade(&screen);
        print.connect_clicked(move |_| {
            if let Some(s) = w.upgrade() {
                s.on_print();
            }
        });
        let w = Rc::downgrade(&screen);
        screen.show_ranking.connect_toggled(move |_| {
            if let Some(s) = w.upgrade() {
                s.refresh();
            }
        });
        screen.refresh();
        screen
    }

    fn with_lobby<R>(&self, f: impl FnOnce(&mut Lobby, &mut bp_core::stage::StageContext) -> R) -> Option<R> {
        let page = self.page.upgrade()?;
        let index = self.index;
        page.with_contest(|c| {
            c.with_schedule(|s, ctx| {
                let stage = s.stage_mut(index)?.as_any_mut().downcast_mut::<Lobby>()?;
                Some(f(stage, ctx))
            })
        })
        .flatten()
    }

    fn changed(&self) {
        if let Some(p) = self.page.upgrade() {
            p.mark_dirty();
            if let Some(view) = p.schedule.borrow().clone() {
                view.refresh_screens();
            }
        }
        self.refresh();
    }

    /// Nouvelle formule ou répartition : les lobbys sont recomposés.
    fn on_composition_changed(&self) {
        if self.updating.get() {
            return;
        }
        let Some(lobbies) = self.formula_values.borrow().get(self.formula.selected() as usize).copied() else { return };
        let distribution = if self.distribution.selected() == 1 { Distribution::Level } else { Distribution::Balanced };
        let changed = self.with_lobby(|l, ctx| {
            if l.base().locked || l.has_results() || (l.settings.lobbies == lobbies && l.settings.distribution == distribution) {
                return false;
            }
            l.set_composition(ctx, lobbies, distribution);
            true
        });
        if changed == Some(true) {
            self.changed();
        }
    }

    fn on_settings_changed(&self) {
        if self.updating.get() {
            return;
        }
        let (lobbies, distribution) = self.with_lobby(|l, _| (l.settings.lobbies, l.settings.distribution)).unwrap_or((1, Distribution::Balanced));
        let scale: Vec<u32> = self.scale.text().split([',', ';', ' ']).filter_map(|v| v.trim().parse().ok()).collect();
        let per_lobby = (self.per_lobby_mode.selected() == 1).then(|| self.per_lobby.value_as_int().max(1) as u32);
        let settings = LobbySettings {
            lobbies,
            distribution,
            races: self.races.value_as_int().max(1) as u32,
            scale,
            abandon_points: self.abandon.value_as_int().max(0) as u32,
            per_lobby,
            swapping: self.swapping.iter().filter(|(_, c)| c.is_active()).map(|(code, _)| code.clone()).collect(),
            team_mode: if self.team_mode.selected() == 1 { TeamMode::Members } else { TeamMode::Team },
            team_rule: if self.team_rule.selected() == 1 { TeamRule::Best } else { TeamRule::Sum },
            team_best: self.team_best.value_as_int().max(1) as u32,
        };
        let qualified = self.qualified.value_as_int().max(0) as u32;
        let changed = self.with_lobby(|l, ctx| {
            if l.base().locked {
                return false;
            }
            let quota = (qualified > 0).then_some(qualified);
            let differs = l.settings != settings || l.base().nb_qualified != quota;
            if differs {
                l.base_mut().nb_qualified = quota;
                l.set_settings(ctx, settings);
            }
            differs
        });
        if changed == Some(true) {
            self.changed();
        }
    }

    fn set_finish(&self, lobby: usize, race: usize, player: PlayerRef, text: &str) {
        let Some(finish) = parse_finish(text) else { return };
        let changed = self.with_lobby(|l, _| {
            if l.base().locked || l.finish(lobby, race, player) == finish {
                return false;
            }
            l.set_finish(lobby, race, player, finish);
            true
        });
        if changed == Some(true) {
            self.changed();
        }
    }

    /// Feuille du lobby affiché (cases vides pour l'arbitre), ou classement.
    fn on_print(&self) {
        let Some(v) = self.read() else { return };
        let Some(page) = self.page.upgrade() else { return };
        let races = v.settings.races.max(1) as usize;
        let (title, headers, rows) = if self.show_ranking.is_active() {
            let headers = vec![tr("Place"), tr("Name"), tr("Club"), tr("Points"), tr("Wins")];
            let rows = v.ranking.iter().map(|(p, n, c, s)| vec![p.to_string(), n.clone(), c.clone(), s.points.to_string(), s.wins.to_string()]).collect();
            (tr("Ranking of the phase"), headers, rows)
        } else {
            let i = (self.lobby_choice.selected() as usize).min(v.lobbies.len().saturating_sub(1));
            let Some(l) = v.lobbies.get(i) else { return };
            let mut headers = vec![tr("Fencer"), tr("Club")];
            headers.extend((1..=races).map(|k| format!("{} {k}", tr("Race"))));
            headers.push(tr("Points"));
            let rows = l
                .rows
                .iter()
                .map(|r| {
                    let mut row = vec![r.name.clone(), r.club.clone()];
                    row.extend(r.finishes.iter().map(|f| finish_text(*f)));
                    row.push(if r.standing.races > 0 { r.standing.points.to_string() } else { String::new() });
                    row
                })
                .collect();
            let title = [format!("Lobby {}", i + 1), l.roadmap.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" — ");
            (title, headers, rows)
        };
        if let Err(e) = super::pool_supervisor::print::classification(page.window().as_ref(), &title, headers, rows) {
            bp_gtk::dialogs::error(page.window().as_ref(), &e.to_string());
        }
    }

    fn read(&self) -> Option<View> {
        let page = self.page.upgrade()?;
        page.with_contest(|c| {
            let l = c.schedule.stage(self.index)?.as_any().downcast_ref::<Lobby>()?;
            let players = &c.players;
            let lobbies = (0..l.lobbies().len())
                .map(|i| {
                    let ranking = l.lobby_ranking(i);
                    let rows = l.lobbies()[i]
                        .iter()
                        .map(|r| {
                            let (name, club) = name_of(players, *r);
                            RowView {
                                player: *r,
                                name,
                                club,
                                finishes: (0..l.settings.races.max(1) as usize).map(|k| l.finish(i, k, *r)).collect(),
                                standing: l.standing(*r),
                                place: ranking.iter().find(|(x, _, _)| x == r).map(|(_, p, _)| *p).unwrap_or(0),
                            }
                        })
                        .collect();
                    let road = l.roadmap(i).cloned().unwrap_or_default();
                    let referees: Vec<String> = road.referees.iter().filter_map(|r| c.referees.store().get(*r)).map(|p| format!("{} {}", p.name(), p.first_name()).trim().to_string()).collect();
                    let roadmap = [Some(road.label()).filter(|s| !s.is_empty()), (!referees.is_empty()).then(|| format!("{} : {}", tr("Referee"), referees.join(", ")))]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    LobbyView { rows, done: l.races_done(i), roadmap }
                })
                .collect();
            let members = l.members_race(players);
            let ranking: Vec<(u32, String, String, Standing)> = l
                .compute_classification(players)
                .into_iter()
                .map(|(r, place)| {
                    let (name, club) = name_of(players, r);
                    let standing = if members { l.team_standing(players, r) } else { l.standing(r) };
                    (place, name, club, standing)
                })
                .collect();
            let individual = if members {
                l.individual_ranking(players)
                    .into_iter()
                    .map(|(r, place, s)| {
                        let (name, club) = name_of(players, r);
                        let team = l.team_of(players, r).and_then(|t| players.get(t)).map(|t| t.name()).unwrap_or(club);
                        (place, name, team, s)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Some(View {
                locked: l.base().locked,
                competitors: l.competitors(players).len(),
                settings: l.settings.clone(),
                team_event: c.settings.team_event,
                nb_qualified: l.base().nb_qualified.unwrap_or(0),
                lobbies,
                ranking,
                individual,
                has_results: l.has_results(),
            })
        })
        .flatten()
    }

    fn header(grid: &gtk::Grid, col: i32, text: &str) {
        let l = gtk::Label::new(None);
        l.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(text)));
        l.set_xalign(0.0);
        l.set_margin_end(10);
        grid.attach(&l, col, 0, 1, 1);
    }

    fn cell(grid: &gtk::Grid, col: i32, row: i32, text: &str, bold: bool) {
        let l = gtk::Label::new(None);
        let escaped = gtk::glib::markup_escape_text(text);
        l.set_markup(&if bold { format!("<b>{escaped}</b>") } else { escaped.to_string() });
        l.set_xalign(0.0);
        l.set_margin_end(10);
        grid.attach(&l, col, row, 1, 1);
    }

    /// Tableau de saisie d'un lobby : une colonne par manche.
    fn lobby_grid(self: &Rc<Self>, lobby: usize, data: &LobbyView, races: usize, locked: bool) -> gtk::Grid {
        let grid = gtk::Grid::new();
        grid.set_row_spacing(4);
        grid.set_column_spacing(4);
        grid.set_margin_start(10);
        grid.set_margin_top(6);
        Self::header(&grid, 0, &tr("Fencer"));
        Self::header(&grid, 1, &tr("Club"));
        for k in 0..races {
            Self::header(&grid, 2 + k as i32, &format!("{} {}", tr("Race"), k + 1));
        }
        Self::header(&grid, 2 + races as i32, &tr("Points"));
        Self::header(&grid, 3 + races as i32, &tr("Wins"));
        Self::header(&grid, 4 + races as i32, &tr("Place"));
        for (i, row) in data.rows.iter().enumerate() {
            let y = i as i32 + 1;
            Self::cell(&grid, 0, y, &row.name, true);
            Self::cell(&grid, 1, y, &row.club, false);
            for k in 0..races {
                let entry = gtk::Entry::new();
                entry.set_width_chars(3);
                entry.set_max_width_chars(3);
                entry.set_text(&finish_text(row.finishes.get(k).copied().flatten()));
                entry.set_sensitive(!locked);
                let w = Rc::downgrade(self);
                let player = row.player;
                entry.connect_activate(move |e| {
                    if let Some(s) = w.upgrade() {
                        s.set_finish(lobby, k, player, &e.text());
                    }
                });
                let focus = gtk::EventControllerFocus::new();
                let w = Rc::downgrade(self);
                let e2 = entry.clone();
                focus.connect_leave(move |_| {
                    if let Some(s) = w.upgrade() {
                        s.set_finish(lobby, k, player, &e2.text());
                    }
                });
                entry.add_controller(focus);
                grid.attach(&entry, 2 + k as i32, y, 1, 1);
            }
            Self::cell(&grid, 2 + races as i32, y, &row.standing.points.to_string(), true);
            Self::cell(&grid, 3 + races as i32, y, &row.standing.wins.to_string(), false);
            Self::cell(&grid, 4 + races as i32, y, &if row.place > 0 && row.standing.races > 0 { row.place.to_string() } else { String::new() }, true);
        }
        grid
    }

    fn ranking_grid(title: &str, list: &[(u32, String, String, Standing)], qualified: u32) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
        b.set_margin_start(10);
        b.set_margin_top(6);
        let t = gtk::Label::new(None);
        t.set_markup(&format!("<b>{}</b>", gtk::glib::markup_escape_text(title)));
        t.set_xalign(0.0);
        b.append(&t);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(2);
        Self::header(&grid, 0, &tr("Place"));
        Self::header(&grid, 1, &tr("Name"));
        Self::header(&grid, 2, &tr("Club"));
        Self::header(&grid, 3, &tr("Points"));
        Self::header(&grid, 4, &tr("Wins"));
        Self::header(&grid, 5, &tr("Best place"));
        for (i, (place, name, club, s)) in list.iter().enumerate() {
            let y = i as i32 + 1;
            let q = qualified > 0 && *place <= qualified;
            Self::cell(&grid, 0, y, &place.to_string(), q);
            Self::cell(&grid, 1, y, name, true);
            Self::cell(&grid, 2, y, club, false);
            Self::cell(&grid, 3, y, &s.points.to_string(), true);
            Self::cell(&grid, 4, y, &s.wins.to_string(), false);
            Self::cell(&grid, 5, y, &if s.best > 0 { s.best.to_string() } else { "—".into() }, false);
        }
        b.append(&grid);
        b
    }
}

impl StageScreen for LobbyScreen {
    fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn configuration(&self) -> Option<gtk::Widget> {
        Some(self.configuration.clone())
    }

    fn refresh(&self) {
        let Some(me) = self.me.borrow().upgrade() else { return };
        let Some(v) = self.read() else { return };
        self.updating.set(true);
        // Réglages
        let s = &v.settings;
        self.races.set_value(s.races as f64);
        if !self.scale.has_focus() {
            self.scale.set_text(&s.scale.iter().map(u32::to_string).collect::<Vec<_>>().join(", "));
        }
        self.abandon.set_value(s.abandon_points as f64);
        self.qualified.set_value(v.nb_qualified as f64);
        self.per_lobby_mode.set_selected(if s.per_lobby.is_some() { 1 } else { 0 });
        self.per_lobby.set_value(s.per_lobby.unwrap_or(1) as f64);
        self.per_lobby.set_sensitive(s.per_lobby.is_some());
        for (code, c) in &self.swapping {
            c.set_active(s.swapping.contains(code));
        }
        self.team_box.set_visible(v.team_event);
        self.team_mode.set_selected(if s.team_mode == TeamMode::Members { 1 } else { 0 });
        self.team_rule.set_selected(if s.team_rule == TeamRule::Best { 1 } else { 0 });
        self.team_best.set_value(s.team_best as f64);
        self.team_best.set_sensitive(s.team_rule == TeamRule::Best);
        self.configuration.set_sensitive(!v.locked);
        // Formule et répartition : modifiables tant qu'aucune arrivée n'est saisie.
        let values = lobby_choices(v.competitors);
        if *self.formula_values.borrow() != values {
            let labels: Vec<String> = values.iter().map(|k| choice_label(*k, v.competitors)).collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            self.formula.set_model(Some(&gtk::StringList::new(&refs)));
            *self.formula_values.borrow_mut() = values.clone();
        }
        if let Some(i) = values.iter().position(|k| *k == s.lobbies) {
            self.formula.set_selected(i as u32);
        }
        self.distribution.set_selected(if s.distribution == Distribution::Level { 1 } else { 0 });
        let composable = !v.locked && !v.has_results && v.competitors > 0;
        self.formula.set_sensitive(composable);
        self.distribution.set_sensitive(composable);
        // Choix du lobby
        let n = v.lobbies.len().max(1);
        if self.lobby_count.get() != n {
            let names: Vec<String> = (1..=n).map(|i| format!("Lobby {i}")).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            self.lobby_choice.set_model(Some(&gtk::StringList::new(&refs)));
            self.lobby_count.set(n);
        }
        let races = s.races.max(1) as usize;
        let done: usize = v.lobbies.iter().filter(|l| l.done >= races).count();
        let current = (self.lobby_choice.selected() as usize).min(v.lobbies.len().saturating_sub(1));
        let road = v.lobbies.get(current).map(|l| l.roadmap.clone()).filter(|r| !r.is_empty()).map(|r| format!(" · {r}")).unwrap_or_default();
        self.summary.set_text(&format!("{} {} · {} {races} · {done}/{} {}{road}", v.lobbies.len(), tr("lobby(ies)"), tr("races:"), v.lobbies.len(), tr("lobby(ies) over")));
        self.updating.set(false);
        // Contenu : saisie du lobby choisi, ou classement de la phase.
        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        if self.show_ranking.is_active() {
            let quota = s.per_lobby.map(|_| 0).unwrap_or(v.nb_qualified);
            body.append(&Self::ranking_grid(&tr("Ranking of the phase"), &v.ranking, quota));
            if !v.individual.is_empty() {
                body.append(&Self::ranking_grid(&tr("Individual ranking"), &v.individual, 0));
            }
        } else {
            let i = (self.lobby_choice.selected() as usize).min(v.lobbies.len().saturating_sub(1));
            if let Some(l) = v.lobbies.get(i) {
                body.append(&me.lobby_grid(i, l, races, v.locked));
            } else {
                let l = gtk::Label::new(Some(&tr("The lobbies are made when the phase starts.")));
                l.add_css_class("bp-muted");
                body.append(&l);
            }
        }
        self.content.set_child(Some(&body));
    }
}
