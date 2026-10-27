//! Feuilles arbitres, affichage des résultats et classements en cairo.

use bp_core::i18n::tr;
use bp_core::rounds::pool::pool::PoolSheet;
use bp_gtk::canvas::{Anchor, Canvas, ItemId, Prop};
use gtk4 as gtk;

fn table(canvas: &Canvas, parent: ItemId) -> ItemId {
    let table = canvas.table(parent);
    canvas.set(
        table,
        [
            Prop::ColumnSpacing(10.0),
            Prop::RowSpacing(5.0),
            Prop::HorzGridLineWidth(0.5),
            Prop::VertGridLineWidth(0.5),
        ],
    );
    table
}

/// Identité d'un tireur telle que l'affiche la feuille : attributs choisis
/// dans « Données » (icônes pour les attributs graphiques, comme le sexe).
#[derive(Clone, Default)]
pub struct Identity {
    pub icons: Vec<gtk::gdk_pixbuf::Pixbuf>,
    /// (code de l'attribut, texte).
    pub texts: Vec<(String, String)>,
}

/// Tout ce qu'il faut pour imprimer des feuilles de poule.
#[derive(Clone)]
pub struct SheetsData {
    /// Nom imprimé dans le bandeau (« Poules - Tour n° 1 »).
    pub page_name: String,
    pub sheets: Vec<PoolSheet>,
    /// QR code de chaque feuille (téléphones des arbitres, avec PIN).
    pub qr: Vec<Option<String>>,
    pub density: (u32, u32),
    pub identities: std::collections::HashMap<bp_core::player::PlayerRef, Identity>,
}

pub fn sheets(parent: Option<&gtk::Window>, data: SheetsData, watermark: bool) -> Result<(), gtk::glib::Error> {
    let job = data.page_name.clone();
    bp_gtk::print::run_document(parent, &job, move || sheets_source(&data, watermark))?;
    Ok(())
}

const CELL: f64 = 45.0;

/// Texte balisé de l'identité, avec les couleurs du C++ (`GetPlayerImage`).
fn identity_markup(identity: &Identity, names_color: &str) -> String {
    let esc = |s: &str| gtk::glib::markup_escape_text(s).to_string();
    identity
        .texts
        .iter()
        .filter(|(_, t)| !t.is_empty())
        .map(|(code, text)| match code.as_str() {
            "name" => format!("<span font_weight=\"bold\" foreground=\"{names_color}\">{}</span>", esc(text)),
            "first_name" => format!("<span foreground=\"{names_color}\">{}</span>", esc(text)),
            "club" | "league" | "region" | "country" => {
                format!("<span style=\"italic\" foreground=\"dimgrey\">{}</span>", esc(text))
            }
            _ => esc(text),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Identité suivie d'un suffixe (`"  3"`) ou précédée d'un préfixe, avec ses
/// icônes à gauche. Renvoie le groupe créé.
fn put_identity(canvas: &Canvas, parent: ItemId, identity: &Identity, before: &str, after: &str, font: &str, names_color: &str) -> ItemId {
    let group = canvas.group(parent);
    let mut x = 0.0;
    if !before.is_empty() {
        let item = canvas.markup(group, before, x, 0.0, -1.0, Anchor::W);
        canvas.set(item, [Prop::Font(font.to_string())]);
        x = canvas.bounds(item).x2 + 4.0;
    }
    for icon in &identity.icons {
        let image = canvas.image(group, Some(icon), x, -icon.height() as f64 / 2.0);
        x = canvas.bounds(image).x2 + 4.0;
    }
    let markup = format!("{}{}", identity_markup(identity, names_color), gtk::glib::markup_escape_text(after));
    let text = canvas.markup(group, &markup, x, 0.0, -1.0, Anchor::W);
    canvas.set(text, [Prop::Font(font.to_string())]);
    group
}

/// Cases à cocher des touches (`Match::GetScoreTable`) : `n` carrés numérotés.
fn score_boxes(canvas: &Canvas, parent: ItemId, n: u32, x: f64, y: f64, size: f64) -> f64 {
    for i in 0..n {
        let bx = x + i as f64 * (size + size / 10.0);
        let r = canvas.rect(parent, bx, y - size / 2.0, size, size);
        canvas.set(r, [Prop::LineWidth(size / 10.0), Prop::StrokeColor("black".into()), Prop::NoFill]);
        let t = canvas.text(parent, &(i + 1).to_string(), bx + size / 2.0, y, -1.0, Anchor::Center);
        canvas.set(t, [Prop::Font(format!("Sans Bold {:.2}px", 0.75 * size)), Prop::FillColor("black".into())]);
    }
    n as f64 * (size + size / 10.0)
}

/// Une feuille de poule (`Pool::Draw` pour l'impression).
fn draw_sheet(canvas: &Canvas, sheet: &PoolSheet, data: &SheetsData, qr: Option<&str>, watermark: bool) -> ItemId {
    let layout = bp_gtk::print::layout();
    let s = layout.body_scale();
    let px = |size: f64| format!("Sans {:.2}px", size * s);
    let bold = |size: f64| format!("Sans Bold {:.2}px", size * s);
    let names_color = layout.names_color.clone();
    let grey = layout.grey_color.clone();
    let root = canvas.group(canvas.root());
    let n = sheet.rows.len();
    let empty = Identity::default();
    let identity_of = |p: bp_core::player::PlayerRef| data.identities.get(&p).unwrap_or(&empty);

    // Nom de la poule
    let title = canvas.text(root, &sheet.title, 0.0, 0.0, -1.0, Anchor::NW);
    canvas.set(title, [Prop::Font(bold(18.0)), Prop::FillColor("black".into())]);
    let title_w = canvas.bounds(title).width();

    // Arbitre et piste
    let box_w = CELL * 5.0;
    let box_y = 34.0 * s;
    let referee_x = title_w + 15.0;
    let piste_x = referee_x + box_w + CELL / 2.0;
    for (x, label) in [(referee_x, tr("Referee")), (piste_x, tr("Piste"))] {
        let r = canvas.rect(root, x, box_y, box_w, CELL);
        canvas.set(r, [Prop::StrokeColor("grey".into()), Prop::LineWidth(2.0), Prop::NoFill]);
        let l = canvas.text(root, &label, x, box_y, -1.0, Anchor::SW);
        canvas.set(l, [Prop::FillColor("grey".into()), Prop::Font(bold(30.0))]);
    }
    if let Some((name, first_name)) = &sheet.referee {
        let t = canvas.text(root, name, referee_x + 5.0, box_y + CELL / 3.0, -1.0, Anchor::W);
        canvas.set(t, [Prop::FillColor("darkgreen".into()), Prop::Font(bold(25.0))]);
        let width = canvas.bounds(t).width();
        if width > box_w - 10.0 { canvas.set(t, [Prop::Font(bold(25.0 * (box_w - 10.0) / width))]); }
        if let Some(f) = first_name {
            let t = canvas.text(root, f, referee_x + 20.0, box_y + 2.4 * CELL / 3.0, -1.0, Anchor::W);
            canvas.set(t, [Prop::FillColor("darkgreen".into()), Prop::Font(bold(15.0))]);
        }
    }
    // QR code de la feuille : l'arbitre le scanne puis entre son PIN.
    if let Some(text) = qr {
        let size = box_y + CELL;
        crate::widgets::score_sheet::draw_qr(canvas, root, text, piste_x + box_w + CELL / 2.0, 0.0, size);
    }
    if let Some(piste) = &sheet.piste {
        let t = canvas.text(root, piste, piste_x + 5.0, box_y + CELL / 2.0, -1.0, Anchor::W);
        canvas.set(t, [Prop::FillColor("darkgreen".into()), Prop::Font(bold(25.0))]);
        let width = canvas.bounds(t).width();
        if width > box_w - 10.0 { canvas.set(t, [Prop::Font(bold(25.0 * (box_w - 10.0) / width))]); }
    }

    // Noms (à gauche de la grille) : on les place d'abord pour connaître leur largeur.
    let grid_top = box_y + CELL + CELL / 2.0 + 28.0 * s;
    let mut name_items = Vec::with_capacity(n);
    let mut names_w: f64 = 0.0;
    for (i, row) in sheet.rows.iter().enumerate() {
        let item = put_identity(canvas, root, identity_of(row.player), "", &format!("\u{a0}\u{a0}{}", i + 1), &px(14.0), &names_color);
        names_w = names_w.max(canvas.bounds(item).width());
        name_items.push(item);
    }
    let grid_x = names_w + 8.0;
    for (i, item) in name_items.into_iter().enumerate() {
        let b = canvas.bounds(item);
        canvas.translate(item, grid_x - 6.0 - b.x2, grid_top + CELL / 2.0 + i as f64 * CELL);
    }

    // Numéros des colonnes
    for j in 0..n {
        let t = canvas.text(root, &(j + 1).to_string(), grid_x + CELL / 2.0 + j as f64 * CELL, grid_top - 13.0, -1.0, Anchor::Center);
        canvas.set(t, [Prop::Font(px(18.0)), Prop::FillColor("black".into())]);
    }

    // Grille
    for (i, row) in sheet.rows.iter().enumerate() {
        for (j, cell) in row.cells.iter().enumerate() {
            let x = grid_x + j as f64 * CELL;
            let y = grid_top + i as f64 * CELL;
            let r = canvas.rect(root, x, y, CELL, CELL);
            canvas.set(
                r,
                [
                    Prop::LineWidth(2.0),
                    Prop::StrokeColor("black".into()),
                    Prop::FillColor(if cell.grey { grey.clone() } else { "white".into() }),
                ],
            );
            if !cell.grey && !cell.text.is_empty() {
                let color = if sheet.for_referees {
                    if watermark {
                        "grey"
                    } else {
                        "white"
                    }
                } else {
                    "black"
                };
                let t = canvas.text(root, &cell.text, x + CELL / 2.0, y + CELL / 2.0, -1.0, Anchor::Center);
                canvas.set(t, [Prop::Font(bold(18.0)), Prop::FillColor(color.into())]);
            }
        }
    }
    let border = canvas.rect(root, grid_x, grid_top, n as f64 * CELL, n as f64 * CELL);
    canvas.set(border, [Prop::LineWidth(5.0), Prop::StrokeColor("black".into()), Prop::NoFill]);

    // Tableau de bord : signatures (arbitres) ou résultats
    let dash_x = grid_x + n as f64 * CELL + CELL / 2.0;
    if sheet.for_referees {
        for i in 0..n {
            let y = grid_top + i as f64 * CELL;
            let r = canvas.rect(root, dash_x, y, box_w, CELL);
            canvas.set(r, [Prop::FillColor("#d9d9d9".into()), Prop::StrokeColor("white".into()), Prop::LineWidth(2.0)]);
            let t = canvas.text(root, &tr("Signature"), dash_x, y, -1.0, Anchor::NW);
            canvas.set(t, [Prop::FillColor("#f2f2f2".into()), Prop::Font(bold(30.0))]);
        }
    } else {
        let headers = [
            tr("Victories"),
            tr("Vict./Matchs"),
            tr("H. scored"),
            tr("H. received"),
            tr("Index"),
            tr("Place"),
        ];
        for (k, h) in headers.iter().enumerate() {
            let x = dash_x + CELL / 2.0 + k as f64 * CELL;
            let y = grid_top - 10.0;
            let t = canvas.text(root, h, x, y, -1.0, Anchor::W);
            canvas.set(t, [Prop::Font(px(18.0)), Prop::FillColor("black".into())]);
            canvas.rotate(t, 315.0, x, y);
        }
        for (i, row) in sheet.rows.iter().enumerate() {
            let Some(d) = &row.dashboard else { continue };
            let y = grid_top + CELL / 2.0 + i as f64 * CELL;
            for (k, v) in [&d.victories, &d.victories_ratio, &d.hits_scored, &d.hits_received, &d.indice, &d.rank].iter().enumerate() {
                let t = canvas.text(root, v, dash_x + CELL / 2.0 + k as f64 * CELL, y, -1.0, Anchor::Center);
                canvas.set(t, [Prop::Font(px(18.0)), Prop::FillColor("black".into())]);
            }
        }
    }

    // Liste des matchs (feuilles d'arbitres)
    if (sheet.for_referees || sheet.matches.iter().any(|m| m.resource.is_some() || !m.officiating.is_empty())) && !sheet.matches.is_empty() {
        let mut top = grid_top + n as f64 * CELL + CELL / 2.0;
        let line_h = 20.0 * s;
        let has_resources = sheet.matches.iter().any(|m| m.resource.is_some());
        if has_resources {
            let instruction = canvas.text(root, &tr("Finish each wave before starting the next one."), 0.0, top, -1.0, Anchor::NW);
            canvas.set(instruction, [Prop::Font(bold(12.0))]);
            top += canvas.bounds(instruction).height() + 8.0 * s;
        }
        let block_h = 2.0 * line_h + CELL / 2.0 + if has_resources { 36.0 * s } else { 0.0 };
        let box_size = 10.0 * s;
        let clock = bp_gtk::icons::icon_pixbuf("clock", 20)
            .or_else(|| gtk::gdk_pixbuf::Pixbuf::from_file_at_size(bp_gtk::resources::image_path("clock.png"), 20, 20).ok());
        // Construit chaque bloc à l'origine, puis les range en colonnes.
        let mut blocks = Vec::new();
        for m in &sheet.matches {
            let block = canvas.group(root);
            let name = canvas.text(block, &m.name, 0.0, line_h, -1.0, Anchor::W);
            canvas.set(name, [Prop::Font(bold(18.0)), Prop::FillColor("black".into())]);
            let mut names_end = canvas.bounds(name).x2 + 4.0;
            if m.rest_error {
                if let Some(c) = clock.as_ref() {
                    let icon = canvas.image(block, Some(c), names_end, line_h - 10.0);
                    names_end = canvas.bounds(icon).x2;
                }
            }
            let players_x = names_end.max(55.0 * s) + 8.0;
            let mut player_items = Vec::new();
            for (k, f) in m.opponents.iter().enumerate() {
                let Some(f) = f else { continue };
                let item = put_identity(
                    canvas,
                    block,
                    identity_of(f.player),
                    &format!("<span font_weight=\"bold\">{}</span>\u{a0}\u{a0}", f.position),
                    "",
                    &px(14.0),
                    &names_color,
                );
                canvas.translate(item, players_x, line_h / 2.0 + k as f64 * line_h);
                player_items.push((item, k));
            }
            let players_end = player_items.iter().map(|(i, _)| canvas.bounds(*i).x2).fold(players_x, f64::max) + 8.0;
            for (_, k) in &player_items {
                score_boxes(canvas, block, m.score_boxes, players_end, line_h / 2.0 + *k as f64 * line_h, box_size);
            }
            if let Some(resource) = &m.resource {
                let label = canvas.text(block, resource, 0.0, 2.0 * line_h + 2.0, -1.0, Anchor::NW);
                canvas.set(label, [Prop::Font(bold(12.0)), Prop::FillColor("black".into())]);
            }
            if !m.officiating.is_empty() {
                let label = canvas.text(block, &format!("Cartons (T1/T2)\n{}", m.officiating), 0.0, 2.0 * line_h + if m.resource.is_some() { 40.0 * s } else { 4.0 * s }, -1.0, Anchor::NW);
                canvas.set(label, [Prop::Font(px(12.0)), Prop::FillColor("black".into())]);
            }
            blocks.push(block);
        }
        let block_h = block_h.max(blocks.iter().map(|b| canvas.bounds(*b).height() + 10.0 * s).fold(0.0_f64, f64::max));
        let block_w = blocks.iter().map(|b| canvas.bounds(*b).width()).fold(1.0_f64, f64::max) + CELL / 4.0;
        let available = grid_x + n as f64 * CELL;
        // Comme le C++ : on ajoute une colonne tant que la liste est moins
        // large que la grille.
        let columns = ((available / block_w).floor() as usize + 1).max(1);
        for (k, b) in blocks.into_iter().enumerate() {
            canvas.translate(b, (k % columns) as f64 * block_w, top + (k / columns) as f64 * block_h);
        }
    }
    root
}

/// Feuilles de poule paginées (aperçu, impression, impression complète).
pub fn sheets_source(data: &SheetsData, watermark: bool) -> Box<dyn bp_gtk::print::PageSource> {
    let canvas = Canvas::new();
    let groups: Vec<ItemId> = data
        .sheets
        .iter()
        .enumerate()
        .map(|(i, sheet)| draw_sheet(&canvas, sheet, data, data.qr.get(i).cloned().flatten().as_deref(), watermark))
        .collect();
    Box::new(bp_gtk::print::Tiled::new(&data.page_name, &data.page_name, canvas, groups, data.density.0 as usize, data.density.1 as usize))
}

pub fn classification(
    parent: Option<&gtk::Window>,
    title: &str,
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
) -> Result<(), gtk::glib::Error> {
    let job = title.to_string();
    let title = title.to_string();
    bp_gtk::print::run_document(parent, &job, move || classification_source(&title, &headers, &rows))?;
    Ok(())
}

/// Classement paginé.
pub fn classification_source(title: &str, headers: &[String], rows: &[Vec<String>]) -> Box<dyn bp_gtk::print::PageSource> {
    let canvas = Canvas::new();
    let mut groups = Vec::new();
    for chunk in rows.chunks(25) {
        let grid = table(&canvas, canvas.root());
        for (j, header) in headers.iter().enumerate() {
            let item = canvas.put_text_in_table(grid, header, 0, j as u32);
            canvas.set(item, [Prop::Font("Sans Bold 14px".into())]);
        }
        for (i, row) in chunk.iter().enumerate() {
            for (j, text) in row.iter().enumerate() {
                canvas.put_text_in_table(grid, text, i as u32 + 1, j as u32);
            }
        }
        groups.push(grid);
    }
    Box::new(bp_gtk::print::Tiled::new(title, "", canvas, groups, 1, 1))
}
