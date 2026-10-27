//! Cadre commun des phases d'une compétition (port de `supervisor/stage.cpp`).
//!
//! En C++, chaque phase est à la fois un objet métier et un module GTK. Ici
//! le trait `Stage` ne porte que la logique ; l'interface (crate
//! `bellepoule`) retrouve le type concret par `as_any()`.
//!
//! Une compétition est une suite de phases ; la sortie (`output`) de l'une
//! devient l'entrée (`attendees`) de la suivante.

use std::any::Any;

use crate::player::{AttrId, Owner, PlayerRef, PlayerStore};
use crate::xml::XmlWriter;

/// Types de phases (`Stage::RegisterStageClass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageKind {
    /// « Check-in » — `Pointage`.
    Checkin,
    /// « Pools arrangement » — composition des poules (fournisseur d'entrée
    /// de `PoolSupervisor`, enregistré dans le même élément `TourDePoules`).
    PoolAllocator,
    /// « Pools » — `TourDePoules`.
    PoolSupervisor,
    /// « Table » — `PhaseDeTableaux`.
    TableSupervisor,
    /// « Barrage » — `Barrage`.
    Barrage,
    /// « Splitting » — `PointDeScission`.
    Splitting,
    /// « General classification » — `ClassementGeneral`.
    GeneralClassification,
    /// « Youth training » — `EntrainementJeunes` (extension du portage Rust).
    YouthTraining,
    /// « Division pools » — `PoulesDeDivision` (extension : montées et descentes).
    DivisionRound,
    /// « Lobby (series) » — `Lobby` (extension : course ou jeu, ordre d'arrivée).
    Lobby,
}

impl StageKind {
    pub const ALL: [StageKind; 10] = [
        StageKind::Checkin,
        StageKind::PoolAllocator,
        StageKind::PoolSupervisor,
        StageKind::TableSupervisor,
        StageKind::Barrage,
        StageKind::Splitting,
        StageKind::YouthTraining,
        StageKind::DivisionRound,
        StageKind::Lobby,
        StageKind::GeneralClassification,
    ];

    /// Nom de balise XML (`_xml_class_name`). Vide pour l'allocateur.
    pub fn xml_name(self) -> &'static str {
        match self {
            StageKind::Checkin => "Pointage",
            StageKind::PoolAllocator => "",
            StageKind::PoolSupervisor => "TourDePoules",
            StageKind::TableSupervisor => "PhaseDeTableaux",
            StageKind::Barrage => "Barrage",
            StageKind::Splitting => "PointDeScission",
            StageKind::GeneralClassification => "ClassementGeneral",
            StageKind::YouthTraining => "EntrainementJeunes",
            StageKind::DivisionRound => "PoulesDeDivision",
            StageKind::Lobby => "Lobby",
        }
    }

    /// Nom affiché (msgid, à traduire avec `i18n::tr`).
    pub fn class_name(self) -> &'static str {
        match self {
            StageKind::Checkin => "Check-in",
            StageKind::PoolAllocator => "Pools arrangement",
            StageKind::PoolSupervisor => "Pools",
            StageKind::TableSupervisor => "Table",
            StageKind::Barrage => "Barrage",
            StageKind::Splitting => "Splitting",
            StageKind::GeneralClassification => "General classification",
            StageKind::YouthTraining => "Youth training",
            StageKind::DivisionRound => "Division pools",
            StageKind::Lobby => "Lobby (series)",
        }
    }

    pub fn from_xml_name(name: &str) -> Option<StageKind> {
        StageKind::ALL.into_iter().find(|k| !k.xml_name().is_empty() && k.xml_name() == name)
    }

    pub fn rights(self) -> StageRights {
        match self {
            StageKind::Checkin | StageKind::Barrage => StageRights { editable: true, removable: false },
            StageKind::PoolSupervisor | StageKind::TableSupervisor | StageKind::Splitting | StageKind::YouthTraining | StageKind::DivisionRound | StageKind::Lobby => {
                StageRights { editable: true, removable: true }
            }
            StageKind::PoolAllocator | StageKind::GeneralClassification => StageRights::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageRights {
    pub editable: bool,
    pub removable: bool,
}

/// Paramètres de la compétition utiles aux phases (sous-ensemble de `Contest`).
#[derive(Debug, Clone)]
pub struct ContestSettings {
    /// Code XML de l'arme : `F`, `E`, `S` ou `X` (éducatif).
    pub weapon: String,
    /// `M`, `F` ou `X` (mixte).
    pub gender: String,
    pub team_event: bool,
    pub minimum_team_size: u32,
    /// Tireurs d'une équipe dans une rencontre (fixé à l'appel).
    pub team_size: u32,
    /// Remplaçants d'une équipe (fixé à l'appel).
    pub substitutes: u32,
    pub default_classification: u32,
    pub manual_classification: bool,
    /// « ScoreAleatoire » : saisie automatique de scores (tests).
    pub score_stuffing_allowed: bool,
}

impl Default for ContestSettings {
    fn default() -> Self {
        ContestSettings {
            weapon: "E".into(),
            gender: "M".into(),
            team_event: false,
            minimum_team_size: 3,
            team_size: 3,
            substitutes: 1,
            default_classification: 10000,
            manual_classification: false,
            score_stuffing_allowed: false,
        }
    }
}

/// Contexte passé aux phases : les joueurs (pour lire/écrire leurs attributs
/// locaux) et les paramètres de la compétition.
pub struct StageContext<'a> {
    pub players: &'a mut PlayerStore,
    pub settings: &'a ContestSettings,
}

/// Erreur bloquante affichée sur une phase (`supervisor/error.cpp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageError {
    pub level: ErrorLevel,
    /// Message déjà traduit.
    pub message: String,
    /// Indication supplémentaire (ex. nom de la poule).
    pub guilty: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorLevel {
    Warning,
    Error,
}

/// Une ligne de classement de phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankEntry {
    pub player: PlayerRef,
    pub rank: u32,
    /// Statut local : `Q` qualifié, `N` non qualifié, `A` abandon,
    /// `E` exclu, `F` forfait.
    pub status: char,
}

/// Phase précédente vue par une phase (`Stage::SetPrevious`) : son
/// identifiant (propriétaire des attributs `rank` qu'elle a calculés) et son
/// dépassement de quota (ex-aequo, utilisé par le barrage).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviousStage {
    pub id: u32,
    pub quota_exceedance: u32,
    /// Score maximum de ses matchs (barrage : même nombre de touches).
    pub max_score: Option<u32>,
    /// Quota de qualifiés (barrage : places à attribuer aux ex-aequo).
    pub nb_qualified: Option<u32>,
}

/// Données communes à toutes les phases (champs privés de `Stage`).
#[derive(Debug, Clone)]
pub struct StageBase {
    pub kind: StageKind,
    /// `PhaseID` dans les fichiers.
    pub id: u32,
    /// `ID` dans les fichiers : nom saisi par l'utilisateur.
    pub name: String,
    pub locked: bool,
    pub rand_seed: u32,
    /// `ScoreMax` (absent pour les phases sans matchs).
    pub max_score: Option<u32>,
    pub bout_timing: crate::officiating::BoutTiming,
    /// `NbQualifies` : nombre de qualifiés pour la phase suivante.
    pub nb_qualified: Option<u32>,
    /// Identifiant réseau (`NetID`, hexadécimal).
    pub net_id: u32,
    /// Participants dans l'ordre d'entrée (rang de départ = index + 1).
    pub attendees: Vec<PlayerRef>,
    /// Colonnes affichées (`Filter`), noms de code d'attributs.
    pub filter: Vec<String>,
    /// Colonnes affichées dans la vue « classement ».
    pub classification_filter: Vec<String>,
    /// Quota dépassé lors du dernier calcul des qualifiés (ex-aequo).
    pub quota_exceedance: u32,
}

impl StageBase {
    pub fn new(kind: StageKind, id: u32) -> Self {
        StageBase {
            kind,
            id,
            name: String::new(),
            locked: false,
            rand_seed: 0,
            max_score: None,
            bout_timing: crate::officiating::BoutTiming { period_seconds: 180, periods: if kind == StageKind::TableSupervisor { 3 } else { 1 }, relay: Default::default() },
            nb_qualified: None,
            net_id: 0,
            attendees: Vec::new(),
            filter: Vec::new(),
            classification_filter: Vec::new(),
            quota_exceedance: 0,
        }
    }

    /// Propriétaire des attributs locaux de cette phase.
    pub fn owner(&self) -> Owner {
        Owner::Stage(self.id)
    }

    /// `Stage::LoadConfiguration` (attributs communs).
    pub fn load_configuration(&mut self, node: roxmltree::Node) {
        self.bout_timing.load(node);
        if let Some(name) = node.attribute("ID") {
            self.name = name.to_string();
        }
        if let Some(v) = node.attribute("ScoreMax") {
            self.max_score = Some(crate::attribute::parse_leading_int(v).max(0) as u32);
        }
        if let Some(v) = node.attribute("NbQualifies") {
            self.nb_qualified = Some(crate::attribute::parse_leading_int(v).max(0) as u32);
        }
        if let Some(v) = node.attribute("NetID") {
            self.net_id = u32::from_str_radix(v, 16).unwrap_or(0);
        }
    }

    /// `Stage::SaveConfiguration` (attributs communs). `phase_id` est l'ID
    /// réellement écrit (l'allocateur écrit celui de la phase suivante).
    pub fn save_configuration(&self, w: &mut XmlWriter, phase_id: u32) {
        self.bout_timing.save(w);
        w.attr_fmt("PhaseID", phase_id);
        w.attr("ID", &self.name);
        if self.net_id != 0 {
            w.attr("NetID", &format!("{:x}", self.net_id));
        }
        if let Some(m) = self.max_score {
            w.attr_fmt("ScoreMax", m);
        }
        if let Some(n) = self.nb_qualified {
            w.attr_fmt("NbQualifies", n);
        }
    }
}

/// Comportement d'une phase.
///
/// Cycle de vie (piloté par `contest`) :
/// 1. création (`new`) puis `load` si on ouvre un fichier ;
/// 2. `set_attendees` quand la phase précédente est terminée et verrouillée ;
/// 3. `garnish` prépare la phase (création des poules, du tableau…) ;
/// 4. la saisie des scores modifie l'état ; `is_over` indique la fin ;
/// 5. `lock` fige la phase et `output` donne les qualifiés.
pub trait Stage: Any + Send {
    fn base(&self) -> &StageBase;
    fn base_mut(&mut self) -> &mut StageBase;

    fn kind(&self) -> StageKind {
        self.base().kind
    }

    /// Phase « fournisseur d'entrée » à créer juste avant celle-ci et chargée
    /// depuis le même élément XML (`GetInputProvider` : l'allocateur de
    /// poules pour `PoolSupervisor`).
    fn input_provider(&self) -> Option<StageKind> {
        None
    }

    /// Vrai pour un fournisseur d'entrée (`GetInputProviderClient`) : il
    /// n'a pas d'élément XML propre et écrit le `PhaseID` de la phase suivante.
    fn is_input_provider(&self) -> bool {
        false
    }

    /// Relie cette phase à son fournisseur d'entrée juste après leur création
    /// (`SetInputProvider`). Le fournisseur est celui de `input_provider()`.
    fn attach_input_provider(&mut self, provider: &mut dyn Stage) {
        let _ = provider;
    }

    /// Les identifiants des phases viennent d'être renumérotés (`SetId`).
    fn id_changed(&mut self) {}

    /// Nom de la phase (`GetName`) : un tour de poules le partage avec son allocateur.
    fn name(&self) -> String {
        self.base().name.clone()
    }

    fn set_name(&mut self, name: &str) {
        self.base_mut().name = name.to_string();
    }

    /// La phase est affichée (`OnPlugged`) : ses données sont reprises de la
    /// phase qui la précède.
    fn on_plugged(&mut self, ctx: &mut StageContext) {
        let _ = ctx;
    }

    /// Score maximum des matchs de la phase.
    fn bout_max_score(&self) -> Option<u32> {
        self.base().max_score
    }

    fn bout_timing(&self) -> crate::officiating::BoutTiming { self.base().bout_timing }

    fn set_bout_timing(&mut self, timing: crate::officiating::BoutTiming) { self.base_mut().bout_timing = timing; }

    /// Quota de qualifiés de la phase.
    fn qualified_quota(&self) -> Option<u32> {
        self.base().nb_qualified
    }

    /// La phase établit son propre classement (`HasItsOwnRanking`) ; faux
    /// pour le classement général, qui reprend celui de la phase précédente.
    fn has_its_own_ranking(&self) -> bool {
        true
    }

    /// Reçoit les participants (sortie de la phase précédente).
    fn set_attendees(&mut self, ctx: &mut StageContext, attendees: Vec<PlayerRef>) {
        let _ = ctx;
        self.base_mut().attendees = attendees;
    }

    /// Prépare la phase à partir de ses participants (`Garnish`).
    fn garnish(&mut self, ctx: &mut StageContext) {
        let _ = ctx;
    }

    /// Annule le travail de la phase (`Reset`) : les participants redeviennent
    /// qualifiés et perdent leur rang final ; les attributs locaux de la
    /// phase sont effacés.
    fn reset(&mut self, ctx: &mut StageContext) {
        default_reset(self.base(), ctx);
    }

    /// Lit l'élément XML de la phase (déjà identifié par sa balise).
    fn load(&mut self, ctx: &mut StageContext, node: roxmltree::Node) -> Result<(), crate::Error>;

    /// Écrit l'élément XML complet de la phase.
    fn save(&self, ctx: &StageContext, w: &mut XmlWriter);

    /// Écrit les paramètres de la phase (sans les participants). Utilisé par
    /// le format de modèle de compétition.
    fn save_configuration(&self, w: &mut XmlWriter) {
        let b = self.base();
        b.save_configuration(w, b.id);
    }

    fn is_over(&self, ctx: &StageContext) -> bool;

    fn error(&self, ctx: &StageContext) -> Option<StageError> {
        let _ = ctx;
        None
    }

    /// Classement de la phase (tous les participants, éliminés compris).
    fn classification(&self, ctx: &StageContext) -> Vec<RankEntry>;

    /// Qualifiés pour la phase suivante, dans l'ordre de classement.
    fn output(&self, ctx: &StageContext) -> Vec<PlayerRef> {
        let mut entries = self.classification(ctx);
        entries.retain(|e| e.status == 'Q');
        let limit = self.base().nb_qualified.map(|n| n as usize).unwrap_or(usize::MAX);
        entries.into_iter().take(limit).map(|e| e.player).collect()
    }

    /// Indique la phase précédente (`SetPrevious`), à appeler par la
    /// compétition avant `set_attendees`/`load`. Les tableaux, le barrage et
    /// la scission s'en servent pour départager par le rang précédent.
    fn set_previous(&mut self, previous: Option<PreviousStage>) {
        let _ = previous;
    }

    fn lock(&mut self, ctx: &mut StageContext) {
        let _ = ctx;
        self.base_mut().locked = true;
    }

    fn unlock(&mut self, ctx: &mut StageContext) {
        let _ = ctx;
        self.base_mut().locked = false;
    }

    /// Renumérotation des joueurs (le pointage réattribue les références
    /// lors de son verrouillage). Par défaut, seuls les participants sont
    /// mis à jour ; une phase qui mémorise d'autres références doit la
    /// surcharger.
    fn remap_players(&mut self, map: &std::collections::HashMap<PlayerRef, PlayerRef>) {
        for r in self.base_mut().attendees.iter_mut() {
            if let Some(n) = map.get(r) {
                *r = *n;
            }
        }
    }

    /// Export HTML de la phase (`Module::DumpToHTML`). Vide par défaut.
    fn dump_to_html(&self, ctx: &StageContext, html: &mut String) {
        let _ = (ctx, html);
    }

    /// Fin du chargement du fichier (`OnLoadingCompleted`).
    fn on_loading_completed(&mut self, ctx: &mut StageContext) {
        let _ = ctx;
    }

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl dyn Stage {
    pub fn downcast_ref<T: Stage>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }

    pub fn downcast_mut<T: Stage>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut::<T>()
    }
}

/// Écrit la liste des participants d'une phase (`Stage::SaveAttendees`) :
/// `<Tireur REF=".." RangInitial=".." RangFinal=".." Statut=".."/>`.
/// Les attributs absents (rang non calculé…) ne sont pas écrits.
/// `data_owner` est le propriétaire des attributs `rank`/`status`
/// (`GetPlayerDataOwner`, ou la phase suivante pour un allocateur).
pub fn save_attendees(base: &StageBase, ctx: &StageContext, w: &mut XmlWriter, data_owner: Owner) {
    use crate::player::AttrId;
    let start_id = AttrId::local("stage_start_rank", base.owner());
    let rank_id = AttrId::local("rank", data_owner);
    let status_id = AttrId::local("status", data_owner);
    for r in &base.attendees {
        let Some(p) = ctx.players.get(*r) else { continue };
        w.start(p.class.xml_tag());
        w.attr_fmt("REF", p.reference);
        if let Some(v) = p.get(&start_id) {
            w.attr_fmt("RangInitial", v.as_uint());
        }
        if let Some(v) = p.get(&rank_id) {
            w.attr_fmt("RangFinal", v.as_uint());
        }
        if let Some(v) = p.get(&status_id) {
            w.attr("Statut", &v.xml_image());
        }
        w.end();
    }
}

/// Lit un participant (`Stage::LoadAttendees`) : renseigne `stage_start_rank`,
/// `status` (local) et `global_status`. Renvoie la référence lue.
pub fn load_attendee(base: &StageBase, ctx: &mut StageContext, node: roxmltree::Node, data_owner: Owner) -> Option<PlayerRef> {
    use crate::player::AttrId;
    let r = crate::attribute::parse_leading_int(node.attribute("REF")?) as PlayerRef;
    let p = ctx.players.get_mut(r)?;
    p.set_uint(&AttrId::local("stage_start_rank", base.owner()), crate::xml::attr_u32(node, "RangInitial", 0));
    let status = node.attribute("Statut").unwrap_or("Q");
    p.set_str(&AttrId::local("status", data_owner), status);
    p.set_str(&AttrId::global("global_status"), status);
    Some(r)
}

/// `Stage::Reset` : voir `Stage::reset`.
pub fn default_reset(base: &StageBase, ctx: &mut StageContext) {
    let final_rank = AttrId::global("final_rank");
    let global_status = AttrId::global("global_status");
    for r in &base.attendees {
        if let Some(p) = ctx.players.get_mut(*r) {
            p.set_str(&global_status, "Q");
            p.remove(&final_rank);
        }
    }
    let owner = base.owner();
    for p in ctx.players.iter_mut() {
        p.remove_owner(owner);
    }
}

/// Phase non encore portée : conserve son élément XML tel quel pour que
/// l'ouverture puis l'enregistrement d'un fichier ne perdent rien.
/// Sert de remplacement pendant le portage, module par module.
pub struct RawStage {
    base: StageBase,
    raw_xml: Option<String>,
}

impl RawStage {
    pub fn new(kind: StageKind, id: u32) -> Self {
        RawStage { base: StageBase::new(kind, id), raw_xml: None }
    }
}

impl Stage for RawStage {
    fn base(&self) -> &StageBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut StageBase {
        &mut self.base
    }

    fn load(&mut self, _ctx: &mut StageContext, node: roxmltree::Node) -> Result<(), crate::Error> {
        self.base.load_configuration(node);
        let src = node.document().input_text();
        self.raw_xml = Some(src[node.range()].to_string());
        Ok(())
    }

    fn save(&self, ctx: &StageContext, w: &mut XmlWriter) {
        match &self.raw_xml {
            Some(xml) => w.raw_indented(xml),
            None if !self.base.kind.xml_name().is_empty() => {
                w.start(self.base.kind.xml_name());
                self.save_configuration(w);
                save_attendees(&self.base, ctx, w, self.base.owner());
                w.end();
            }
            None => {}
        }
    }

    fn is_over(&self, _ctx: &StageContext) -> bool {
        false
    }

    fn classification(&self, _ctx: &StageContext) -> Vec<RankEntry> {
        self.base
            .attendees
            .iter()
            .enumerate()
            .map(|(i, r)| RankEntry { player: *r, rank: i as u32 + 1, status: 'Q' })
            .collect()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
