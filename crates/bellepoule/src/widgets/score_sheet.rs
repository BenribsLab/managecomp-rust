//! Feuille de match imprimée (`TableSet::DrawPage`, cas `SCORE_SHEETS`, et
//! `DrawPlayerMatch`) : nom du match, cadres arbitre et piste, puis pour
//! chaque tireur son identité, une case par touche à cocher par l'arbitre
//! (`Match::GetScoreTable`), la case du score et la case de signature.
//! Partagée par les tableaux et les matchs de barrage.

use bp_core::i18n::tr;
use bp_core::player::{PlayerRef, PlayerStore};
use bp_gtk::canvas::{Anchor, Canvas, Prop};

/// Hauteur de police des impressions (`PRINT_FONT_HEIGHT`, % de la largeur).
const FONT: f64 = 2.0;
/// Largeur de la feuille dessinée (unités du C++ : % de la largeur du papier).
const SHEET_WIDTH: f64 = 120.0;

pub struct SheetInfo {
    pub name: String,
    pub officiating: String,
    /// Nom et prénom de l'arbitre.
    pub referee: Option<(String, String)>,
    /// « 03 @ 14:20 ».
    pub piste: Option<String>,
    /// Identité (balisage Pango) des deux tireurs.
    pub fencers: Vec<String>,
    pub max_score: u32,
    /// Référence du match (remplace le code flash).
    pub flash_ref: Option<String>,
    /// QR code du match (téléphone de l'arbitre, avec PIN).
    pub qr: Option<String>,
}

/// QR code dessiné en carrés noirs (net à l'impression), coin haut gauche
/// en (`x`, `y`), côté `size`.
pub fn draw_qr(canvas: &Canvas, parent: bp_gtk::canvas::ItemId, text: &str, x: f64, y: f64, size: f64) {
    let Some(m) = bp_net::flash_code::QrMatrix::encode(text) else { return };
    let module = size / m.width as f64;
    let background = canvas.rect(parent, x, y, size, size);
    canvas.set(background, [Prop::FillColor("white".into()), Prop::NoStroke]);
    for yy in 0..m.width {
        // Carrés noirs voisins d'une même ligne réunis.
        let mut xx = 0;
        while xx < m.width {
            if !m.is_dark(xx, yy) {
                xx += 1;
                continue;
            }
            let start = xx;
            while xx < m.width && m.is_dark(xx, yy) {
                xx += 1;
            }
            let r = canvas.rect(parent, x + start as f64 * module, y + yy as f64 * module, (xx - start) as f64 * module + module * 0.02, module * 1.02);
            canvas.set(r, [Prop::FillColor("black".into()), Prop::NoStroke]);
        }
    }
}

/// Identité d'un tireur comme `GetPlayerImage` : nom en gras bleu foncé,
/// prénom bleu foncé, club, ligue, région, pays en italique gris.
pub fn fencer_markup(players: &PlayerStore, r: PlayerRef) -> String {
    let Some(p) = players.get(r) else { return String::new() };
    let esc = |s: &str| gtk4::glib::markup_escape_text(s).to_string();
    let mut parts = Vec::new();
    if let Some(v) = p.str_attr("name").filter(|v| !v.is_empty()) {
        parts.push(format!("<span font_weight=\"bold\" foreground=\"darkblue\">{}</span>", esc(v)));
    }
    if let Some(v) = p.str_attr("first_name").filter(|v| !v.is_empty()) {
        parts.push(format!("<span foreground=\"darkblue\">{}</span>", esc(v)));
    }
    for code in ["club", "league", "region", "country"] {
        if let Some(v) = p.str_attr(code).filter(|v| !v.is_empty()) {
            parts.push(format!("<span style=\"italic\" foreground=\"dimgrey\">{}</span>", esc(v)));
        }
    }
    parts.join("  ")
}

fn font(size: f64, bold: bool) -> String {
    format!("Sans {}{size:.2}px", if bold { "Bold " } else { "" })
}

/// Dessine une feuille (largeur `SHEET_WIDTH` unités).
pub fn sheet_canvas(info: &SheetInfo) -> Canvas {
    let canvas = Canvas::new();
    let root = canvas.root();
    let title_font = font(3.5 / 2.0 * FONT, true);
    let small_font = font(FONT, true);

    // Nom du match
    let name = canvas.text(root, &info.name, 0.0, 4.0, -1.0, Anchor::W);
    canvas.set(name, [Prop::Font(title_font.clone()), Prop::FillColor("grey".into())]);
    if let Some(r) = &info.flash_ref {
        let t = canvas.text(root, r, 0.0, 8.5, -1.0, Anchor::W);
        canvas.set(t, [Prop::Font(font(1.4, false)), Prop::FillColor("grey".into())]);
    }

    // Cadres arbitre et piste
    let frame = |x: f64, label: &str| {
        let rect = canvas.rect(root, x, 1.0, 35.0, 6.0);
        canvas.set(rect, [Prop::StrokeColor("grey70".into()), Prop::LineWidth(0.2), Prop::NoFill]);
        let l = canvas.text(root, label, x, 1.0, -1.0, Anchor::W);
        canvas.set(l, [Prop::Font(small_font.clone()), Prop::FillColor("grey".into())]);
    };
    frame(30.0, &tr("Referee"));
    if let Some((last, first)) = &info.referee {
        let n = canvas.text(root, last, 30.0, 3.6, -1.0, Anchor::W);
        canvas.set(n, [Prop::Font(title_font.clone()), Prop::FillColor("DarkGreen".into())]);
        let f = canvas.text(root, first, 32.0, 6.0, -1.0, Anchor::W);
        canvas.set(f, [Prop::Font(small_font.clone()), Prop::FillColor("DarkGreen".into())]);
    }
    frame(85.0, &tr("Piste"));
    if let Some(text) = &info.qr {
        draw_qr(&canvas, root, text, 122.0, 0.0, 15.0);
    }
    if let Some(piste) = &info.piste {
        let p = canvas.text(root, piste, 90.0, 4.2, -1.0, Anchor::W);
        canvas.set(p, [Prop::Font(title_font.clone()), Prop::FillColor("DarkGreen".into())]);
    }

    // Une ligne par tireur
    let name_width = 40.0;
    // Une case par touche (au-delà de 15 touches, l'original n'en dessine
    // pas ; ici elles passent sur plusieurs rangées de 15).
    let boxes = info.max_score.min(60);
    let per_row = boxes.min(15);
    let box_rows = boxes.div_ceil(15).max(1);
    // Cases élargies dès qu'un numéro a deux chiffres, rangées espacées.
    let box_w = if boxes >= 10 { FONT * 1.6 } else { FONT };
    let hpitch = box_w + FONT / 10.0;
    let pitch = FONT + FONT / 4.0;
    let boxes_x = name_width + 1.0;
    let score_x = boxes_x + per_row as f64 * hpitch + 1.0;
    let signature_x = score_x + 4.0 * 2.0 + 1.0;
    let row_height = (box_rows as f64 * pitch + 2.0).max(6.0);
    for (row, identity) in info.fencers.iter().enumerate() {
        let y = 19.0 + row as f64 * row_height;
        let first_box_y = y - (box_rows as f64 - 1.0) * pitch / 2.0;
        let n = canvas.markup(root, identity, 0.0, y, name_width, Anchor::W);
        canvas.set(n, [Prop::Font(font(FONT, false)), Prop::Ellipsize(gtk4::pango::EllipsizeMode::End)]);
        for i in 0..boxes {
            let x = boxes_x + (i % 15) as f64 * hpitch;
            let by = first_box_y + (i / 15) as f64 * pitch;
            let rect = canvas.rect(root, x, by - FONT / 2.0, box_w, FONT);
            canvas.set(rect, [Prop::LineWidth(FONT / 10.0), Prop::NoFill]);
            let t = canvas.text(root, &(i + 1).to_string(), x + box_w / 2.0, by, -1.0, Anchor::Center);
            canvas.set(t, [Prop::Font(font(1.5 / 2.0 * FONT, true))]);
        }
        let score = canvas.rect(root, score_x, y - 2.0, 8.0, 4.0);
        canvas.set(score, [Prop::LineWidth(0.3), Prop::NoFill]);
        let signature = canvas.rect(root, signature_x, y - 2.0, 32.0, 4.0);
        canvas.set(signature, [Prop::FillColor("grey85".into()), Prop::NoStroke]);
        let t = canvas.text(root, &tr("Signature"), signature_x + 16.0, y, -1.0, Anchor::Center);
        canvas.set(t, [Prop::Font(font(3.5, true)), Prop::FillColor("grey95".into())]);
    }
    if !info.officiating.is_empty() {
        let text = canvas.text(root, &format!("Cartons (T1/T2)\n{}", info.officiating), 0.0, 22.0 + info.fencers.len() as f64 * row_height, SHEET_WIDTH, Anchor::NW);
        canvas.set(text, [Prop::Font(font(1.6, false))]);
    }
    canvas
}

/// Dessine sur une page les feuilles `sheets` (une par tranche de hauteur),
/// séparées par un trait de coupe.
pub fn draw_page(cr: &cairo::Context, width: f64, height: f64, sheets: &[SheetInfo], per_page: usize) {
    let slot = height / per_page.max(1) as f64;
    for (i, info) in sheets.iter().enumerate() {
        let canvas = sheet_canvas(info);
        let bounds = canvas.scene_bounds();
        let factor = (width / bounds.width().max(SHEET_WIDTH)).min((slot - 10.0) / bounds.height().max(1.0));
        let _ = cr.save();
        cr.translate(-bounds.x1 * factor, i as f64 * slot - bounds.y1 * factor + 4.0);
        cr.scale(factor, factor);
        canvas.render(cr, None, 1.0);
        let _ = cr.restore();
        if i + 1 < sheets.len() {
            cr.set_source_rgb(1.0, 0.0, 1.0);
            cr.set_line_width(0.5);
            cr.set_dash(&[3.0, 0.8], 0.0);
            cr.move_to(0.0, (i + 1) as f64 * slot - 3.0);
            cr.line_to(width, (i + 1) as f64 * slot - 3.0);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
    }
}
