//! Compétition (port de `supervisor/contest.cpp`, partie logique).
//!
//! `Contest` possède tout ce qui compose une compétition : ses propriétés
//! (arme, sexe, catégorie, date…), les tireurs et équipes (`players`), les
//! arbitres (`referees`) et la suite des phases (`schedule`). Il sait lire
//! et écrire le fichier `.cotcot`.
//!
//! L'interface modifie directement les champs publics puis appelle
//! `apply_properties` (équivalent de `ReadProperties`). Elle appelle
//! `mark_dirty` après toute modification : l'enregistrement automatique
//! (5 s après la dernière modification, `SAVE_DELAY_SECONDS`) est à sa charge.

pub mod grand;
pub mod html;
pub mod players;
pub mod referees;
pub mod teams;
pub mod zipper;

use std::path::{Path, PathBuf};

use crate::attribute::{self, Scope};
use crate::formats::askfred::reader::{AskFredFile, Event as AskFredEvent};
use crate::category::Category;
use crate::attribute::Look;
use crate::player::{Player, PlayerClass, PlayerRef, PlayerStore};
use crate::schedule::Schedule;
use crate::stage::{ContestSettings, StageContext};
use crate::tournament::user_config::UserConfig;
use crate::weapon::Weapon;
use crate::xml::{self, XmlWriter};
use crate::{Error, Result};

use referees::RefereesList;

/// Version écrite dans les fichiers (`By BellePoule (V5.7)`).
pub const VERSION: &str = "5";
pub const VERSION_REVISION: &str = "7";
pub const VERSION_MATURITY: &str = "";

/// Délai d'enregistrement automatique après une modification.
pub const SAVE_DELAY_SECONDS: u64 = 5;

/// Sexes : image (msgid), code XML, alias lus pour les anciens fichiers.
pub const GENDER_IMAGES: [&str; 3] = ["Men", "Women", "Mixed"];
pub const GENDER_XML_IMAGES: [&str; 3] = ["M", "F", "X"];
pub const GENDER_XML_ALIASES: [&str; 3] = ["M", "D", "FM"];

/// Niveaux : image (msgid) et code XML.
pub const LEVEL_IMAGES: [&str; 7] =
    ["League", "Regional", "Open", "World cup", "National championships", "World championships", "Other"];
pub const LEVEL_XML_IMAGES: [&str; 7] = ["L", "Z", "CN", "A", "F", "IE", "X"];

/// Couleurs proposées aux compétitions (`Contest::Init`), tour à tour.
pub const COLORS: [&str; 6] = ["#EED680", "#E0B6AF", "#ADA7C8", "#9DB8D2", "#83A67F", "#DF421E"];

/// Heure de la journée (`Contest::Time`) : appel, scratch, début.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContestTime {
    pub hour: u32,
    pub minute: u32,
}

impl Default for ContestTime {
    fn default() -> Self {
        ContestTime { hour: 12, minute: 0 }
    }
}

impl ContestTime {
    /// Image XML `hh:mm`.
    pub fn xml_image(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// `Time::Load` : `hh:mm` ; toute autre forme est ignorée.
    pub fn load(&mut self, text: &str) {
        let mut parts = text.splitn(2, ':');
        if let (Some(h), Some(m)) = (parts.next(), parts.next()) {
            self.hour = attribute::parse_leading_int(h).max(0) as u32;
            self.minute = attribute::parse_leading_int(m).max(0) as u32;
        }
    }
}

pub struct Contest {
    /// Fédération ou championnat (`Championnat`).
    pub authority: Option<String>,
    /// Identifiant FIE (`ID`).
    pub fie_id: Option<String>,
    /// Titre long (`TitreLong`).
    pub name: String,
    pub organizer: String,
    /// Points de la compétition (préférences, partie « Points »).
    pub points: crate::points::PointsSettings,
    /// Origine du fichier : `AskFred…` pour un import, sinon `None`.
    pub source: Option<String>,
    /// Publication sur ManageComp (compete.managecomp.org) : identifiant
    /// court de la page publique, partagé par le logiciel et la version web.
    pub public_id: Option<String>,
    /// Événement publié qui regroupe cette épreuve avec d'autres (les
    /// compétitions ouvertes ensemble), même principe que `public_id`.
    pub public_event: Option<String>,
    pub location: String,
    /// Libellé qui distingue deux compétitions de même arme/sexe/catégorie.
    pub label: String,
    pub web_site: String,
    /// Couleur `#rrggbb` de l'onglet.
    pub color: String,
    pub category: Category,
    /// Index dans `LEVEL_XML_IMAGES`.
    pub level: usize,
    pub weapon: &'static Weapon,
    /// Index dans `GENDER_XML_IMAGES`.
    pub gender: usize,
    pub day: u32,
    pub month: u32,
    pub year: u32,
    pub checkin_time: ContestTime,
    pub scratch_time: ContestTime,
    pub start_time: ContestTime,
    /// Identifiant réseau.
    pub net_id: u32,
    pub filename: Option<PathBuf>,
    /// Paramètres vus par les phases (équipes, classements, scores aléatoires…).
    pub settings: ContestSettings,
    pub players: PlayerStore,
    pub referees: RefereesList,
    pub schedule: Schedule,
    /// Dupliquée depuis une autre compétition (`_derived`).
    pub derived: bool,
    pub read_only: bool,
    /// Des modifications ne sont pas enregistrées.
    pub dirty: bool,
    /// Attributs de joueurs inconnus du registre, déclarés par le fichier
    /// (`GetUnknownAttributes`).
    pub(crate) extra_attributes: Vec<String>,
}

impl Contest {
    /// Nouvelle compétition vide (sans phases). Les valeurs par défaut du
    /// titre, de l'organisateur, du site et du lieu viennent des
    /// préférences ; la couleur est la suivante de la liste (`ChooseColor`).
    pub fn new(config: Option<&mut UserConfig>, for_duplication: bool) -> Contest {
        let today = chrono::Local::now().date_naive();
        use chrono::Datelike;
        let get = |c: &Option<&mut UserConfig>, key: &str| c.as_ref().and_then(|c| c.get_string("Competiton", key)).unwrap_or_default();
        let mut config = config;
        let name = get(&config, "default_name");
        let organizer = get(&config, "default_organizer");
        let web_site = get(&config, "default_web_site");
        let location = get(&config, "default_location");
        let color = Self::choose_color(config.as_deref_mut());

        let settings = ContestSettings {
            weapon: Weapon::default_weapon().xml_image.to_string(),
            gender: GENDER_XML_IMAGES[0].to_string(),
            team_event: false,
            minimum_team_size: 3,
            team_size: 3,
            substitutes: 1,
            default_classification: 9999,
            manual_classification: for_duplication,
            score_stuffing_allowed: false,
        };
        Contest {
            authority: None,
            fie_id: None,
            public_id: None,
            public_event: None,
            name,
            organizer,
            points: crate::points::PointsSettings::default(),
            source: None,
            location,
            label: String::new(),
            web_site,
            color,
            category: Category::new(),
            level: 0,
            weapon: Weapon::default_weapon(),
            gender: 0,
            day: today.day(),
            month: today.month(),
            year: today.year() as u32,
            checkin_time: ContestTime::default(),
            scratch_time: ContestTime::default(),
            start_time: ContestTime::default(),
            net_id: rand::random::<u32>(),
            filename: None,
            settings,
            players: PlayerStore::new(),
            referees: RefereesList::new(),
            schedule: Schedule::new(),
            derived: false,
            read_only: false,
            dirty: false,
            extra_attributes: Vec::new(),
        }
    }

    /// Nouvelle compétition avec ses phases par défaut (pointage, poules,
    /// tableaux, classement) : le cas « Nouvelle compétition ».
    pub fn new_with_default_stages(config: Option<&mut UserConfig>) -> Contest {
        let mut c = Contest::new(config, false);
        c.sync_settings();
        c.referees.set_weapon(c.weapon);
        c.create_default_schedule(false);
        c
    }

    /// `ChooseColor` : couleur suivante, mémorisée dans les préférences.
    pub fn choose_color(config: Option<&mut UserConfig>) -> String {
        let mut index = 0usize;
        if let Some(cfg) = config {
            let mut i = cfg.get_integer("Competiton", "color_to_use").max(0) as usize;
            if i >= COLORS.len() {
                i = 0;
            }
            i += 1;
            if i >= COLORS.len() {
                i = 0;
            }
            cfg.set_integer("Competiton", "color_to_use", i as i64);
            index = i;
        }
        COLORS[index].to_string()
    }

    // ----------------------------------------------------------------------
    // Contexte des phases

    /// Recopie les propriétés utiles aux phases dans `settings`.
    pub fn sync_settings(&mut self) {
        self.settings.weapon = self.weapon.xml_image.to_string();
        self.settings.gender = GENDER_XML_IMAGES[self.gender.min(2)].to_string();
    }

    /// Appelle `f` avec la suite des phases et le contexte (joueurs et
    /// paramètres) qu'elles attendent.
    pub fn with_schedule<R>(&mut self, f: impl FnOnce(&mut Schedule, &mut StageContext) -> R) -> R {
        let mut ctx = StageContext { players: &mut self.players, settings: &self.settings };
        f(&mut self.schedule, &mut ctx)
    }

    /// `CreateDefault`.
    pub fn create_default_schedule(&mut self, without_pools: bool) {
        self.with_schedule(|schedule, ctx| schedule.create_default(ctx, without_pools));
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    // ----------------------------------------------------------------------
    // Accesseurs

    pub fn is_team_event(&self) -> bool {
        self.settings.team_event
    }

    pub fn has_askfred_entry(&self) -> bool {
        self.source.is_some()
    }

    fn is_askfred_source(&self) -> bool {
        self.source.as_deref().map(|s| s.starts_with("AskFred")).unwrap_or(false)
    }

    pub fn gender_code(&self) -> &'static str {
        GENDER_XML_IMAGES[self.gender.min(2)]
    }

    /// Sexe traduit (`GetGender`).
    pub fn gender_label(&self) -> String {
        crate::i18n::tr(GENDER_IMAGES[self.gender.min(2)])
    }

    pub fn level_label(&self) -> String {
        crate::i18n::tr(LEVEL_IMAGES[self.level.min(6)])
    }

    pub fn category_label(&self) -> String {
        self.category.display_image()
    }

    /// Date au format local français (`%x`).
    pub fn date_image(&self) -> String {
        format!("{:02}/{:02}/{}", self.day, self.month, self.year)
    }

    /// Date `jj.mm.aaaa` des fichiers.
    pub fn xml_date(&self) -> String {
        format!("{:02}.{:02}.{}", self.day, self.month, self.year)
    }

    /// `GetDefaultFileName` : `titre-arme-sexe-catégorie[-libellé].cotcot`.
    pub fn default_file_name(&self) -> String {
        let mut s = format!("{}-{}-{}-{}", self.name, self.weapon.label(), self.gender_label(), self.category.display_image());
        if !self.label.is_empty() {
            s.push('-');
            s.push_str(&self.label);
        }
        s.push_str(".cotcot");
        s
    }

    pub fn referee(&self, r: PlayerRef) -> Option<&Player> {
        self.referees.get(r)
    }

    /// `AddFencer`.
    pub fn add_fencer(&mut self, fencer: Player) -> Option<PlayerRef> {
        self.with_schedule(|schedule, ctx| schedule.checkin_mut().map(|c| c.add(ctx, fencer)))
    }

    /// `AddFencer (fencer, rank)` : le classement est renseigné avant l'ajout.
    pub fn add_fencer_with_rank(&mut self, mut fencer: Player, rank: u32) -> Option<PlayerRef> {
        fencer.set_uint(&crate::player::AttrId::global("ranking"), rank);
        self.add_fencer(fencer)
    }

    /// `ReadProperties` (partie logique) : à appeler une fois les champs
    /// modifiés par l'interface. Mémorise les valeurs par défaut dans les
    /// préférences.
    pub fn apply_properties(&mut self, config: Option<&mut UserConfig>) {
        self.label = self.label.trim().to_string();
        self.sync_settings();
        self.referees.set_weapon(self.weapon);
        if let Some(cfg) = config {
            for (key, value) in [
                ("default_name", &self.name),
                ("default_organizer", &self.organizer),
                ("default_web_site", &self.web_site),
                ("default_location", &self.location),
            ] {
                if !value.is_empty() {
                    cfg.set_string("Competiton", key, value);
                }
            }
        }
        self.with_schedule(|schedule, ctx| {
            if let Some(c) = schedule.checkin_mut() {
                c.apply_config(ctx);
            }
        });
        self.mark_dirty();
    }

    /// `ReadTeamProperty`.
    pub fn set_team_event(&mut self, team_event: bool) {
        if self.settings.team_event != team_event {
            self.settings.team_event = team_event;
            self.mark_dirty();
        }
    }

    /// `SetScoreStuffingPolicy`.
    pub fn set_score_stuffing(&mut self, allowed: bool) {
        self.settings.score_stuffing_allowed = allowed;
    }

    /// Texte affiché par la liste des dernières compétitions et le titre de la fenêtre.
    pub fn display_name(&self) -> String {
        match &self.filename {
            Some(f) => f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            None => self.default_file_name(),
        }
    }

    // ----------------------------------------------------------------------
    // Lecture

    /// `LoadXmlFile` : lit un fichier `.cotcot` (ou une compétition FIE
    /// « Base »). L'encodage est déduit de l'en-tête XML. Le nom du fichier
    /// n'est conservé que pour l'extension `.cotcot` : les autres formats
    /// obligent à choisir un nouveau nom à l'enregistrement.
    pub fn load_file(&mut self, path: &Path) -> Result<()> {
        let bytes = std::fs::read(path)?;
        let text = crate::formats::encoding::decode_xml(&bytes)?;
        self.load_xml_str(&text)?;
        let is_cotcot = path.extension().map(|e| e.eq_ignore_ascii_case("cotcot")).unwrap_or(false);
        self.filename = if is_cotcot { Some(absolute(path)) } else { None };
        self.dirty = false;
        Ok(())
    }

    /// `LoadXmlString`.
    pub fn load_xml_str(&mut self, text: &str) -> Result<()> {
        let doc = xml::parse(text)?;
        self.load_doc(&doc)?;
        self.dirty = false;
        Ok(())
    }

    /// `LoadXmlDoc`.
    fn load_doc(&mut self, doc: &roxmltree::Document) -> Result<()> {
        let root = doc.root_element();
        let (team_event, need_post_processing) = match root.tag_name().name() {
            "CompetitionIndividuelle" => (false, false),
            "BaseCompetitionIndividuelle" => (false, true),
            "CompetitionParEquipes" => (true, false),
            "BaseCompetitionParEquipes" => (true, true),
            other => return Err(Error::Format(format!("racine XML inattendue : {other}"))),
        };
        self.settings.team_event = team_event;

        self.read_header(root);
        self.extra_attributes = unknown_attributes(root);
        self.sync_settings();
        self.referees.set_weapon(self.weapon);
        self.referees.load(root, &self.extra_attributes);

        let askfred = self.is_askfred_source();
        {
            let mut ctx = StageContext { players: &mut self.players, settings: &self.settings };
            self.schedule.load(&mut ctx, root, &self.extra_attributes)?;
        }

        self.with_schedule(|schedule, ctx| {
            if let Some(checkin) = schedule.checkin_mut() {
                if askfred {
                    checkin.set_column_look("league", Look::ShortText);
                }
                if need_post_processing {
                    checkin.convert_from_base_to_result(ctx);
                }
                if askfred {
                    checkin.show_column("rating");
                }
            }
            schedule.on_loading_completed(ctx);
        });
        Ok(())
    }

    /// Lit les attributs de l'élément racine.
    fn read_header(&mut self, root: roxmltree::Node) {
        if let Some(c) = root.attribute("Couleur") {
            if is_color(c) {
                self.color = c.to_string();
            }
        }
        if let Some(v) = root.attribute("Championnat") {
            self.authority = Some(v.to_string());
        }
        if let Some(v) = root.attribute("ID") {
            self.fie_id = Some(v.to_string());
        }
        if let Some(v) = root.attribute("NetID") {
            self.net_id = u32::from_str_radix(v.trim(), 16).unwrap_or(self.net_id);
        }
        if let Some(v) = root.attribute("TitreLong") {
            self.name = v.to_string();
        }
        if let Some(v) = root.attribute("Date").or_else(|| root.attribute("DateDebut")) {
            let parts: Vec<&str> = v.splitn(3, '.').collect();
            if parts.len() == 3 {
                self.day = attribute::parse_leading_int(parts[0]).max(0) as u32;
                self.month = attribute::parse_leading_int(parts[1]).max(0) as u32;
                self.year = attribute::parse_leading_int(parts[2]).max(0) as u32;
            }
        }
        if let Some(v) = root.attribute("Appel") {
            self.checkin_time.load(v);
        }
        if let Some(v) = root.attribute("Scratch") {
            self.scratch_time.load(v);
        }
        if let Some(v) = root.attribute("Debut") {
            self.start_time.load(v);
        }
        if let Some(w) = root.attribute("Arme").and_then(Weapon::from_xml) {
            self.weapon = w;
        }
        if let Some(v) = root.attribute("Sexe") {
            if let Some(i) = GENDER_XML_IMAGES.iter().position(|g| *g == v).or_else(|| GENDER_XML_ALIASES.iter().position(|g| *g == v)) {
                self.gender = i;
            }
        }
        if let Some(v) = root.attribute("Categorie") {
            self.category.parse_xml(v);
        }
        if let Some(v) = root.attribute("Niveau") {
            if let Some(i) = LEVEL_XML_IMAGES.iter().position(|l| *l == v) {
                self.level = i;
            }
        }
        if let Some(v) = root.attribute("Organisateur") {
            self.organizer = v.to_string();
        }
        if let Some(v) = root.attribute("Source") {
            self.source = Some(v.to_string());
        }
        if let Some(v) = root.attribute("URLorganisateur") {
            self.web_site = v.to_string();
        }
        if let Some(v) = root.attribute("Lieu") {
            self.location = v.to_string();
        }
        if let Some(v) = root.attribute("Label") {
            self.label = v.to_string();
        }
        self.settings.score_stuffing_allowed = xml::attr_bool(root, "ScoreAleatoire", false);
        self.public_id = root.attribute("PublicationManageComp").filter(|v| !v.is_empty()).map(str::to_string);
        self.public_event = root.attribute("EvenementManageComp").filter(|v| !v.is_empty()).map(str::to_string);
        self.settings.minimum_team_size = xml::attr_u32(root, "TailleMinimaleEquipe", self.settings.minimum_team_size);
        self.settings.team_size = xml::attr_u32(root, "TireursParEquipe", self.settings.team_size).clamp(1, 20);
        self.settings.substitutes = xml::attr_u32(root, "RemplacantsParEquipe", self.settings.substitutes).min(10);
        self.settings.default_classification = xml::attr_u32(root, "ClassementDefautEquipe", self.settings.default_classification);
        self.settings.manual_classification = xml::attr_bool(root, "ClassementManuel", self.settings.manual_classification);
        self.points = root.attribute("Points").and_then(|v| serde_json::from_str::<crate::points::PointsSettings>(v).ok()).map(|p| p.upgraded()).unwrap_or_default();
    }

    /// `LoadFencerFile` : nouvelle compétition à partir d'une liste de
    /// tireurs (`.fff`, `.csv`, `.txt`). Les phases par défaut sont créées ;
    /// l'interface demande ensuite les propriétés (`AskForSettings`).
    pub fn load_fencer_file(&mut self, path: &Path) -> Result<()> {
        let path = absolute(path);
        if !path.is_file() {
            return Ok(());
        }
        self.sync_settings();
        self.referees.set_weapon(self.weapon);
        self.create_default_schedule(false);
        self.with_schedule(|schedule, ctx| match schedule.checkin_mut() {
            Some(checkin) => checkin.import(ctx, &path).map(|_| ()).map_err(Error::from),
            None => Ok(()),
        })?;
        self.mark_dirty();
        Ok(())
    }

    /// `LoadAskFred` : une épreuve d'un fichier AskFred (`.frd`).
    pub fn load_askfred(&mut self, file: &AskFredFile, event: &AskFredEvent) {
        self.name = event.name.clone().unwrap_or_default();
        self.location = event.location.clone().unwrap_or_default();
        self.organizer = event.organizer.clone().unwrap_or_default();
        self.fie_id = event.event_id.clone();
        self.source = Some(event.source());
        if let Some(g) = event.gender.as_deref() {
            if let Some(i) = GENDER_XML_IMAGES.iter().position(|x| *x == g).or_else(|| GENDER_XML_ALIASES.iter().position(|x| *x == g)) {
                self.gender = i;
            }
        }
        if let Some(w) = event.weapon.as_deref().and_then(Weapon::from_xml) {
            self.weapon = w;
        }
        if let Some(c) = &event.category {
            self.category.parse_xml(c);
        }
        self.year = event.year;
        self.month = event.month;
        self.day = event.day;
        let [checkin, scratch, start] = event.times();
        self.checkin_time = ContestTime { hour: checkin.0, minute: checkin.1 };
        self.scratch_time = ContestTime { hour: scratch.0, minute: scratch.1 };
        self.start_time = ContestTime { hour: start.0, minute: start.1 };
        self.settings.team_event = event.team_event;
        self.sync_settings();
        self.referees.set_weapon(self.weapon);

        self.create_default_schedule(false);
        let list = file.event_players(event);
        self.with_schedule(|schedule, ctx| {
            let Some(checkin) = schedule.checkin_mut() else { return };
            checkin.set_column_look("league", Look::ShortText);
            checkin.show_column("rating");
            let mut added: Vec<PlayerRef> = Vec::new();
            for loaded in list.entries {
                let owner = loaded.owner.and_then(|i| added.get(i).copied());
                let r = checkin.add(ctx, loaded.player);
                if let Some(team) = owner {
                    teams::set_team(ctx.players, r, Some(team));
                }
                added.push(r);
            }
            checkin.on_list_changed(ctx);
        });
        self.mark_dirty();
    }

    // ----------------------------------------------------------------------
    // Enregistrement

    /// `SaveHeader` : prologue, élément racine, attributs, puis les listes
    /// de tireurs, d'équipes et d'arbitres. L'élément racine reste ouvert.
    pub(crate) fn save_header(&mut self, w: &mut XmlWriter) {
        let root = if self.settings.team_event { "CompetitionParEquipes" } else { "CompetitionIndividuelle" };
        w.dtd(root);
        w.comment(&format!(
            "\n           By ManageComp (V{VERSION}.{VERSION_REVISION}{VERSION_MATURITY}), derived from BellePoule\n\n   https://managecomp.org\n"
        ));
        w.start(root);
        w.attr("Couleur", &self.color);
        if let Some(v) = &self.authority {
            w.attr("Championnat", v);
        }
        if let Some(v) = &self.fie_id {
            w.attr("ID", v);
        }
        w.attr("NetID", &format!("{:x}", self.net_id));
        w.attr_fmt("Annee", self.year);
        w.attr("Arme", self.weapon.xml_image);
        w.attr("Sexe", self.gender_code());
        w.attr("Organisateur", &self.organizer);
        if let Some(v) = &self.source {
            w.attr("Source", v);
        }
        w.attr("Categorie", &self.category.xml_image());
        w.attr("Niveau", LEVEL_XML_IMAGES[self.level.min(6)]);
        w.attr("Date", &self.xml_date());
        w.attr("Appel", &self.checkin_time.xml_image());
        w.attr("Scratch", &self.scratch_time.xml_image());
        w.attr("Debut", &self.start_time.xml_image());
        w.attr("TitreLong", &self.name);
        w.attr("URLorganisateur", &self.web_site);
        w.attr_fmt("ScoreAleatoire", self.settings.score_stuffing_allowed as u32);
        if let Some(id) = &self.public_id {
            w.attr("PublicationManageComp", id);
        }
        if let Some(id) = &self.public_event {
            w.attr("EvenementManageComp", id);
        }
        w.attr_fmt("TailleMinimaleEquipe", self.settings.minimum_team_size);
        if self.settings.team_event {
            w.attr_fmt("TireursParEquipe", self.settings.team_size);
            w.attr_fmt("RemplacantsParEquipe", self.settings.substitutes);
        }
        w.attr_fmt("ClassementDefautEquipe", self.settings.default_classification);
        w.attr_fmt("ClassementManuel", self.settings.manual_classification as u32);
        if self.points != crate::points::PointsSettings::default() {
            w.attr("Points", &serde_json::to_string(&self.points).unwrap_or_default());
        }
        w.attr("Lieu", &self.location);
        w.attr("Label", &self.label);

        // `SavePeoples`
        let extra = self.extra_attributes.clone();
        let ctx = StageContext { players: &mut self.players, settings: &self.settings };
        if let Some(checkin) = self.schedule.checkin() {
            checkin.save_list(&ctx, w, PlayerClass::Fencer, &extra);
            checkin.save_list(&ctx, w, PlayerClass::Team, &extra);
            self.referees.save(w, &extra);
        }
    }

    /// Document XML complet (`Contest::Save (filename)`).
    pub fn to_xml(&mut self) -> String {
        let mut w = XmlWriter::document();
        self.save_header(&mut w);
        {
            let ctx = StageContext { players: &mut self.players, settings: &self.settings };
            self.schedule.save(&ctx, &mut w);
        }
        w.finish()
    }

    /// Écrit le document dans `path` (écriture dans un fichier temporaire
    /// puis renommage : un plantage ne détruit pas l'ancien fichier).
    pub fn save_to(&mut self, path: &Path) -> Result<()> {
        let text = self.to_xml();
        write_atomically(path, text.as_bytes())?;
        Ok(())
    }

    /// `Save ()` : fichier de la compétition, plus copies facultatives (dossier
    /// de diffusion web et dossier de sauvegarde du tournoi). Sans nom de
    /// fichier, ne fait rien.
    pub fn save(&mut self, www_dir: Option<&Path>, backup_dir: Option<&Path>) -> Result<()> {
        let Some(filename) = self.filename.clone() else { return Ok(()) };
        self.save_to(&filename)?;
        if let Some(base) = filename.file_name() {
            for dir in [www_dir, backup_dir].into_iter().flatten() {
                self.save_to(&dir.join(base))?;
            }
        }
        self.dirty = false;
        Ok(())
    }

    /// `Archive` : enregistre puis ajoute une copie à l'archive `.zip`
    /// du fichier (appelé à chaque changement de phase).
    pub fn archive(&mut self, title: &str, www_dir: Option<&Path>, backup_dir: Option<&Path>) -> Result<()> {
        self.save(www_dir, backup_dir)?;
        if let Some(filename) = &self.filename {
            if let Some(zip) = zipper::archive_path(filename) {
                zipper::archive(&zip, filename, title)?;
            }
        }
        Ok(())
    }

    /// `Duplicate` : nouvelle compétition de mêmes propriétés, sans tireurs
    /// ni tour de poules, pour y reporter les tableaux d'une autre catégorie.
    pub fn duplicate(&self, label: Option<&str>, config: Option<&mut UserConfig>) -> Contest {
        self.duplicate_with(label, config, true)
    }

    fn duplicate_with(&self, label: Option<&str>, config: Option<&mut UserConfig>, without_pools: bool) -> Contest {
        let mut c = Contest::new(config, true);
        c.derived = true;
        if let Some(l) = label {
            c.label = l.to_string();
        }
        c.authority = self.authority.clone();
        c.fie_id = self.fie_id.clone();
        c.name = self.name.clone();
        c.organizer = self.organizer.clone();
        c.points = self.points.clone();
        c.source = self.source.clone();
        c.web_site = self.web_site.clone();
        c.location = self.location.clone();
        c.category.replicate(&self.category);
        c.level = self.level;
        c.weapon = self.weapon;
        c.gender = self.gender;
        c.day = self.day;
        c.month = self.month;
        c.year = self.year;
        c.settings.team_event = self.settings.team_event;
        c.settings.minimum_team_size = self.settings.minimum_team_size;
        c.settings.team_size = self.settings.team_size;
        c.settings.substitutes = self.settings.substitutes;
        c.checkin_time = self.checkin_time;
        c.scratch_time = self.scratch_time;
        c.start_time = self.start_time;
        c.sync_settings();
        c.referees.set_weapon(c.weapon);
        c.create_default_schedule(without_pools);
        c
    }

    /// `Splitting::OnLocked` : copie des participants d'une division (1 =
    /// première), avec les positions de départ de la scission et les membres
    /// des équipes. La compétition prend le nom de la division.
    pub fn duplicate_split(&self, index: usize, division: u32, config: Option<&mut UserConfig>) -> Result<Contest> {
        let splitting = self.schedule.stage(index)
            .and_then(|s| s.as_any().downcast_ref::<crate::rounds::splitting::Splitting>())
            .ok_or_else(|| Error::Format("la phase choisie n'est pas un point de scission".into()))?;
        let label = if splitting.config.mode == crate::rounds::splitting::SplitMode::Manual
            && splitting.config.divisions.len() <= 1
            && splitting.config.divisions.first().is_none_or(|d| d.name == "Division 2")
        {
            // Une seule division, sans nom choisi : le nom de la phase (comme BellePoule).
            crate::stage::Stage::base(splitting).name.clone()
        } else {
            splitting.division_name(division)
        };
        let mut target = self.duplicate(Some(&label), config);
        // Publication : la division rejoint l'événement de la compétition
        // (elle sera publiée à son tour).
        target.public_event = self.public_event.clone();
        // Division par catégories : la compétition créée prend la catégorie
        // (la plus âgée s'il y en a plusieurs ; les autres sont surclassés).
        if let Some(code) = splitting.division_category(division) {
            target.category.parse_xml(&code);
        }
        // Nom : celui de la compétition, suivi de la division.
        if !label.trim().is_empty() {
            target.name = if self.name.trim().is_empty() { label.clone() } else { format!("{} - {}", self.name.trim(), label.trim()) };
        }
        for (reference, rank) in splitting.exported_players(&self.players, division) {
            let Some(player) = self.players.get(reference) else { continue };
            let mut copy = split_player(player);
            copy.set_uint(&crate::player::AttrId::global("ranking"), rank);
            let new_ref = target.add_fencer(copy).ok_or_else(|| Error::Format("impossible d'ajouter le participant extrait".into()))?;
            if player.class == crate::player::PlayerClass::Team {
                for member in &player.members {
                    if let Some(member) = self.players.get(*member) {
                        let mut copy = split_player(member);
                        copy.set_str(&crate::player::AttrId::global("team"), &player.name());
                        target.add_fencer(copy).ok_or_else(|| Error::Format("impossible d'ajouter un membre de l'équipe extraite".into()))?;
                    }
                }
                // L'ajout des membres peut recalculer le classement d'équipe.
                if let Some(team) = target.players.get_mut(new_ref) {
                    team.set_uint(&crate::player::AttrId::global("ranking"), rank);
                }
            }
        }
        // La division poursuit la formule de la compétition après le point de
        // scission (phases et réglages), pas la formule par défaut.
        let continuation = self.formula_after(index);
        if !continuation.is_empty() {
            target.with_schedule(|schedule, ctx| {
                while schedule.len() > 1 {
                    let last = schedule.len() - 1;
                    schedule.remove_stage(ctx, last);
                }
                let mut last_kind = None;
                for spec in &continuation {
                    // Une poule de division ajoute elle-même son tour de poules.
                    let at = if spec.kind == crate::stage::StageKind::PoolSupervisor && last_kind == Some(crate::stage::StageKind::DivisionRound) {
                        schedule.len() - 1
                    } else {
                        schedule.add_kind(ctx, spec.kind, None)
                    };
                    last_kind = Some(spec.kind);
                    if let Some(stage) = schedule.stage_mut(at) {
                        spec.apply(stage, ctx.players);
                    }
                }
                if schedule.stage(schedule.len() - 1).is_none_or(|s| s.kind() != crate::stage::StageKind::GeneralClassification) {
                    schedule.add_kind(ctx, crate::stage::StageKind::GeneralClassification, None);
                }
                schedule.set_current(0);
            });
        }
        target.with_schedule(|schedule, ctx| {
            if let Some(checkin) = schedule.checkin_mut() {
                checkin.on_list_changed(ctx);
            }
        });
        target.dirty = true;
        Ok(target)
    }

    /// Phases de la formule après `index` (point de scission), avec leurs
    /// réglages : ce que les divisions poursuivent.
    fn formula_after(&self, index: usize) -> Vec<StageSpec> {
        use crate::rounds::pool::PoolSupervisorStage;
        use crate::rounds::table::supervisor::TableSupervisor;
        let mut out = Vec::new();
        for j in index + 1..self.schedule.len() {
            let Some(s) = self.schedule.stage(j) else { continue };
            if s.is_input_provider() {
                continue;
            }
            let base = s.base();
            let mut spec = StageSpec {
                kind: s.kind(),
                name: base.name.clone(),
                max_score: base.max_score,
                nb_qualified: base.nb_qualified,
                timing: base.bout_timing,
                pool: None,
                fenced: None,
                lobby: s.as_any().downcast_ref::<crate::rounds::lobby::Lobby>().map(|l| l.settings.clone()),
            };
            if let Some(p) = s.as_any().downcast_ref::<PoolSupervisorStage>() {
                let round = p.round();
                spec.pool = Some(PoolSpec { name: round.name.clone(), max_score: round.max_score, nb_qualified: round.nb_qualified(), criteria: round.swapping_criteria().to_vec() });
            }
            if let Some(t) = s.as_any().downcast_ref::<TableSupervisor>() {
                spec.fenced = Some(t.fenced_places_mode());
                spec.max_score = Some(t.max_score());
            }
            out.push(spec);
        }
        out
    }

    /// Points de chaque tireur au classement général (préférences « Points »
    /// activées et compétition classée) : (tireur, place, points).
    pub fn final_points(&self) -> Vec<(PlayerRef, u32, f64)> {
        if !self.points.enabled {
            return Vec::new();
        }
        let Some(gc) = self.schedule.stages().find_map(|s| s.as_any().downcast_ref::<crate::rounds::classification::GeneralClassificationStage>()) else {
            return Vec::new();
        };
        let ranked: Vec<(PlayerRef, u32)> = gc
            .result()
            .iter()
            .filter_map(|r| {
                let place = self.players.get(*r)?.get(&crate::player::AttrId::global("final_rank"))?.as_uint();
                (place > 0).then_some((*r, place))
            })
            .collect();
        let participants = ranked.len() as u32;
        ranked.into_iter().filter_map(|(r, place)| Some((r, place, self.points.points(participants, place)?))).collect()
    }

    /// Divisions utilisées au point de scission `index` (1 = première).
    pub fn split_divisions(&self, index: usize) -> Vec<u32> {
        self.schedule
            .stage(index)
            .and_then(|s| s.as_any().downcast_ref::<crate::rounds::splitting::Splitting>())
            .map(|s| s.divisions_in_use(&self.players))
            .unwrap_or_default()
    }

    /// Un libellé est conseillé pour la publication sur GREG.
    pub fn needs_label_for_greg(&self) -> bool {
        self.label.is_empty()
    }
}

/// Réglages d'une phase recopiés dans une division (point de scission).
struct StageSpec {
    kind: crate::stage::StageKind,
    name: String,
    max_score: Option<u32>,
    nb_qualified: Option<u32>,
    timing: crate::officiating::BoutTiming,
    pool: Option<PoolSpec>,
    fenced: Option<crate::rounds::table::supervisor::FencedPlaces>,
    /// Lobby (série) : ses réglages (lobbys, manches, barème…).
    lobby: Option<crate::rounds::lobby::LobbySettings>,
}

struct PoolSpec {
    name: String,
    max_score: u32,
    nb_qualified: Option<u32>,
    criteria: Vec<String>,
}

impl StageSpec {
    fn apply(&self, stage: &mut dyn crate::stage::Stage, players: &mut PlayerStore) {
        {
            let base = stage.base_mut();
            if !self.name.is_empty() {
                base.name = self.name.clone();
            }
            base.max_score = self.max_score;
            base.nb_qualified = self.nb_qualified;
            base.bout_timing = self.timing;
        }
        if let (Some(pool), Some(p)) = (&self.pool, stage.as_any_mut().downcast_mut::<crate::rounds::pool::PoolSupervisorStage>()) {
            let mut round = p.round();
            round.name = pool.name.clone();
            round.set_max_score(pool.max_score);
            round.set_nb_qualified(pool.nb_qualified);
            let regular: Vec<&str> = crate::rounds::pool::round::regular_swapping_criteria();
            let reg: Vec<&str> = pool.criteria.iter().map(String::as_str).filter(|c| regular.contains(c)).collect();
            let extras: Vec<&str> = pool.criteria.iter().map(String::as_str).filter(|c| !regular.contains(c)).collect();
            round.set_swapping_criteria(players, &reg, &extras);
        }
        if let (Some(settings), Some(l)) = (&self.lobby, stage.as_any_mut().downcast_mut::<crate::rounds::lobby::Lobby>()) {
            l.settings = settings.clone();
        }
        if let (Some(mode), Some(t)) = (self.fenced, stage.as_any_mut().downcast_mut::<crate::rounds::table::supervisor::TableSupervisor>()) {
            t.set_fenced_places(mode);
            if let Some(m) = self.max_score {
                t.set_max_score(m);
            }
        }
    }
}

impl Contest {
    /// Résultats AskFred avec le magasin d'arbitres distinct.
    pub fn export_askfred(&self) -> String {
        let header = self.header();
        let checkin = self.schedule.checkin();
        let people = crate::formats::fie_xml::XmlPeople {
            fencers: checkin.map(|c| c.fencers(&self.players)).unwrap_or_default(),
            teams: checkin.map(|c| c.teams(&self.players)).unwrap_or_default(),
            referees: self.referees.refs().to_vec(),
        };
        let mut phases = crate::xml::XmlWriter::new().without_indent();
        let mut players = self.players.clone();
        let ctx = StageContext { players: &mut players, settings: &self.settings };
        self.schedule.save(&ctx, &mut phases);
        let phases = phases.finish();
        crate::formats::askfred::writer::dump_to_frd_with_referees(
            &crate::formats::askfred::writer::FrdInput { header: &header, store: &self.players, people: &people, phases_xml: Some(&phases) },
            self.referees.store(),
        )
    }

    /// En-tête de la compétition pour les imports et exports (`formats`).
    pub fn header(&self) -> crate::formats::header::ContestHeader {
        crate::formats::header::ContestHeader {
            team_event: self.settings.team_event,
            color: Some(self.color.clone()),
            authority: self.authority.clone(),
            fie_id: self.fie_id.clone(),
            net_id: Some(self.net_id as u64),
            name: self.name.clone(),
            day: self.day,
            month: self.month,
            year: self.year,
            checkin_time: Some(self.checkin_time.xml_image()),
            scratch_time: Some(self.scratch_time.xml_image()),
            start_time: Some(self.start_time.xml_image()),
            weapon: self.weapon.xml_image.to_string(),
            gender: self.gender_code().to_string(),
            category: self.category.xml_image(),
            category_display: Some(self.category.display_image()),
            level: Some(LEVEL_XML_IMAGES[self.level.min(6)].to_string()),
            organizer: self.organizer.clone(),
            source: self.source.clone(),
            web_site: self.web_site.clone(),
            location: self.location.clone(),
            label: self.label.clone(),
            score_stuffing: self.settings.score_stuffing_allowed,
            minimum_team_size: Some(self.settings.minimum_team_size),
            default_classification: Some(self.settings.default_classification),
            manual_classification: Some(self.settings.manual_classification as u32),
            raw_attributes: Vec::new(),
        }
    }

    /// `DumpToHTML` : page web de la compétition (un onglet par phase).
    pub fn dump_to_html(&mut self) -> String {
        self.dump_to_html_with(None)
    }

    /// Export HTML ; `classification` remplace le contenu du classement
    /// général (entraînement des jeunes : classement de chaque groupe).
    pub fn dump_to_html_with(&mut self, classification: Option<String>) -> String {
        self.dump_to_html_scope(classification, false)
    }

    /// Export HTML ; `final_only` : le classement final seul, sans les poules
    /// ni les tableaux.
    pub fn dump_to_html_scope(&mut self, classification: Option<String>, final_only: bool) -> String {
        let header = self.header();
        let current = self.schedule.current_index();
        let ctx = StageContext { players: &mut self.players, settings: &self.settings };
        let mut rounds = Vec::new();
        for (index, stage) in self.schedule.stages().enumerate() {
            if stage.is_input_provider() {
                continue;
            }
            if final_only && stage.kind() != crate::stage::StageKind::GeneralClassification {
                continue;
            }
            let mut html = String::new();
            if index <= current {
                match (&classification, stage.kind()) {
                    (Some(c), crate::stage::StageKind::GeneralClassification) => html.push_str(c),
                    _ => stage.dump_to_html(&ctx, &mut html),
                }
            }
            rounds.push(crate::formats::html::HtmlRound {
                id: stage.base().id,
                index,
                class_name: crate::i18n::tr(stage.kind().class_name()),
                name: stage.name(),
                html,
            });
        }
        crate::formats::html::dump_contest(&header, &rounds, current)
    }

    /// Classement final au format FFF (`GeneralClassification` → `DumpToFFF`).
    /// `place_shifting` : décalage des places (place de départ − 1).
    pub fn export_final_classification_fff(&self, place_shifting: u32) -> Option<Vec<u8>> {
        let gc = self
            .schedule
            .stages()
            .find_map(|s| s.as_any().downcast_ref::<crate::rounds::classification::GeneralClassificationStage>())?;
        Some(crate::formats::fff::export(&self.header(), &self.players, gc.result(), place_shifting))
    }

    /// Liste des tireurs au format CSV (modèle d'import).
    pub fn export_fencers_csv(&self, delimiter: char) -> String {
        let refs: Vec<PlayerRef> = self.schedule.checkin().map(|c| c.fencers(&self.players)).unwrap_or_default();
        crate::formats::csv::export(&self.players, &refs, delimiter)
    }
}

fn split_player(player: &Player) -> Player {
    use crate::attribute::Value;
    use crate::player::AttrId;
    let mut copy = player.duplicate();
    copy.set_ref(0);
    copy.team = None;
    copy.members.clear();
    copy.set(&AttrId::global("exported"), Value::Bool(false));
    copy.remove(&AttrId::global("final_rank"));
    copy.remove(&AttrId::global("incident"));
    copy.set_str(&AttrId::global("global_status"), if copy.attending() { "Q" } else { "F" });
    copy
}

/// `GetUnknownAttributes` : attributs du premier `<Tireur>` que le registre
/// ne connaît pas ; ils sont conservés tels quels (texte).
fn unknown_attributes(root: roxmltree::Node) -> Vec<String> {
    let registry = attribute::registry();
    let Some(first) = xml::child(root, "Tireurs").and_then(|t| xml::child(t, "Tireur")) else {
        return Vec::new();
    };
    first
        .attributes()
        .map(|a| a.name().to_string())
        .filter(|name| registry.by_xml(name, Scope::Global).is_none())
        .collect()
}

fn is_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit())
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Écrit `data` dans `path` sans jamais laisser un fichier à moitié écrit.
pub fn write_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let temp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&temp, data)?;
    if let Err(e) = std::fs::rename(&temp, path) {
        // Windows refuse parfois de remplacer un fichier ouvert ailleurs.
        let result = std::fs::write(path, data);
        let _ = std::fs::remove_file(&temp);
        return result.or(Err(e));
    }
    Ok(())
}

/// Décode un document XML : UTF-8, ou ISO-8859-1 si déclaré ou invalide.
pub fn decode_xml_bytes(bytes: &[u8]) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_ascii_lowercase();
    if !head.contains("iso-8859-1") {
        if let Ok(s) = std::str::from_utf8(bytes) {
            return s.to_string();
        }
    }
    bytes.iter().map(|b| *b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_roundtrip() {
        let mut t = ContestTime::default();
        t.load("08:45");
        assert_eq!(t.xml_image(), "08:45");
        t.load("abc");
        assert_eq!(t.xml_image(), "08:45");
    }

    #[test]
    fn default_file_name_includes_label() {
        let mut c = Contest::new(None, false);
        c.name = "Open".into();
        c.label = "A".into();
        assert!(c.default_file_name().starts_with("Open-"));
        assert!(c.default_file_name().ends_with("-A.cotcot"));
    }

    #[test]
    fn colors_cycle() {
        let mut cfg = UserConfig::new();
        let a = Contest::choose_color(Some(&mut cfg));
        let b = Contest::choose_color(Some(&mut cfg));
        assert_ne!(a, b);
    }
}
