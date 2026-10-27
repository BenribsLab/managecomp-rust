//! Suite des phases d'une compétition (port de `supervisor/schedule.cpp`,
//! partie logique).
//!
//! `Schedule` possède les phases dans l'ordre d'exécution. Les phases qui
//! ont un fournisseur d'entrée (`Stage::input_provider`) sont précédées de
//! celui-ci (ex. l'allocateur de poules avant le tour de poules).
//!
//! Les phases ont besoin des joueurs et des paramètres de la compétition :
//! `Contest` les possède et passe un `StageContext` à chaque appel.

use std::collections::HashMap;

use crate::contest::grand::stage_rand_seed;
use crate::player::{AttrId, Owner, PlayerRef};
use crate::rounds::{self, checkin::CheckinStage, classification::GeneralClassificationStage};
use crate::stage::{PreviousStage, Stage, StageContext, StageKind};
use crate::xml::XmlWriter;
use crate::Error;

/// Ce que l'utilisateur peut faire à un instant donné (`RefreshSensitivity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sensitivity {
    /// Passer à la phase suivante (la phase courante est terminée).
    pub next: bool,
    /// Revenir à la phase précédente.
    pub previous: bool,
}

#[derive(Default)]
pub struct Schedule {
    stages: Vec<Box<dyn Stage>>,
    current_stage: usize,
    /// Chargement d'un fichier en cours : le pointage diffère sa renumérotation.
    loading: bool,
}

impl Schedule {
    pub fn new() -> Self {
        Schedule { stages: Vec::new(), current_stage: 0, loading: false }
    }

    // ----------------------------------------------------------------------
    // Accès

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    pub fn stage(&self, index: usize) -> Option<&dyn Stage> {
        self.stages.get(index).map(|s| s.as_ref())
    }

    pub fn stage_mut(&mut self, index: usize) -> Option<&mut (dyn Stage + 'static)> {
        self.stages.get_mut(index).map(|s| s.as_mut())
    }

    pub fn stages(&self) -> impl Iterator<Item = &dyn Stage> {
        self.stages.iter().map(|s| s.as_ref())
    }

    pub fn current_index(&self) -> usize {
        self.current_stage
    }

    pub fn current(&self) -> Option<&dyn Stage> {
        self.stage(self.current_stage)
    }

    pub fn current_mut(&mut self) -> Option<&mut (dyn Stage + 'static)> {
        let i = self.current_stage;
        self.stage_mut(i)
    }

    /// Index de la phase d'identifiant `id` (les identifiants sont les index).
    pub fn index_of_id(&self, id: u32) -> Option<usize> {
        self.stages.iter().position(|s| s.base().id == id)
    }

    /// `GetCheckinSupervisor` : le pointage est toujours la phase 0.
    pub fn checkin(&self) -> Option<&CheckinStage> {
        self.stages.first().and_then(|s| s.as_any().downcast_ref::<CheckinStage>())
    }

    pub fn checkin_mut(&mut self) -> Option<&mut CheckinStage> {
        self.stages.first_mut().and_then(|s| s.as_any_mut().downcast_mut::<CheckinStage>())
    }

    /// Dernière phase = classement général : la compétition est terminée.
    pub fn is_over(&self) -> bool {
        if self.stages.is_empty() || self.current_stage + 1 < self.stages.len() {
            return false;
        }
        self.stages[self.current_stage].as_any().is::<GeneralClassificationStage>()
    }

    // ----------------------------------------------------------------------
    // Création

    /// `CreateDefault` : pointage, tour de poules, tableaux, classement.
    pub fn create_default(&mut self, ctx: &mut StageContext, without_pools: bool) {
        if !self.stages.is_empty() {
            return;
        }
        self.add_kind(ctx, StageKind::Checkin, None);
        {
            // `RetrieveAttendees` + `Garnish` + `Display` du pointage.
            self.retrieve_attendees(ctx, 0);
            self.stages[0].garnish(ctx);
        }
        if !without_pools {
            self.add_kind(ctx, StageKind::PoolSupervisor, None);
        }
        self.add_kind(ctx, StageKind::TableSupervisor, None);
        self.add_kind(ctx, StageKind::GeneralClassification, None);
        self.set_current(0);
    }

    pub fn create_stage(&self, kind: StageKind) -> Box<dyn Stage> {
        rounds::create_stage(kind, 0)
    }

    /// `AddStage` avec une phase du type donné. Renvoie l'index de la phase
    /// (le fournisseur d'entrée éventuel est inséré juste avant).
    pub fn add_kind(&mut self, ctx: &mut StageContext, kind: StageKind, after: Option<usize>) -> usize {
        let stage = self.create_stage(kind);
        let at = self.add_stage(ctx, stage, after);
        // Poules de division : le tour de poules de ces divisions suit.
        if kind == StageKind::DivisionRound && !self.loading {
            self.add_kind(ctx, StageKind::PoolSupervisor, Some(at));
        }
        at
    }

    /// `AddStage` : insère la phase après `after` (à la fin par défaut).
    pub fn add_stage(&mut self, ctx: &mut StageContext, stage: Box<dyn Stage>, after: Option<usize>) -> usize {
        let _ = ctx;
        let mut at = match after {
            Some(a) => a + 1,
            None => self.stages.len(),
        }
        .min(self.stages.len());
        // Le classement général reste la dernière phase : une phase ajoutée
        // derrière lui est placée juste avant (sinon « Tour suivant » ne mène
        // nulle part).
        if !self.loading && stage.kind() != StageKind::GeneralClassification {
            if let Some(last) = self.stages.len().checked_sub(1) {
                if self.stages[last].kind() == StageKind::GeneralClassification && at > last {
                    at = last;
                }
            }
        }

        let mut stage = stage;
        if let Some(provider_kind) = stage.input_provider() {
            let mut provider = self.create_stage(provider_kind);
            stage.attach_input_provider(provider.as_mut());
            self.stages.insert(at, provider);
            at += 1;
        }
        self.stages.insert(at, stage);
        if self.loading {
            if let Some(c) = self.stages[at].as_any_mut().downcast_mut::<CheckinStage>() {
                c.set_loading(true);
            }
        }
        self.after_structure_change();
        self.give_name(at);
        at
    }

    /// `GiveStagesAnId` + `SetPrevious` : à rappeler après toute insertion
    /// ou suppression.
    fn after_structure_change(&mut self) {
        for (i, s) in self.stages.iter_mut().enumerate() {
            s.base_mut().id = i as u32;
        }
        for s in self.stages.iter_mut() {
            s.id_changed();
        }
        self.refresh_previous();
    }

    /// Informe chaque phase de sa voisine précédente.
    pub fn refresh_previous(&mut self) {
        let mut previous: Option<PreviousStage> = None;
        for s in self.stages.iter_mut() {
            s.set_previous(previous);
            previous = Some(PreviousStage {
                id: s.base().id,
                quota_exceedance: s.base().quota_exceedance,
                max_score: s.bout_max_score(),
                nb_qualified: s.qualified_quota(),
            });
        }
    }

    /// `GiveName` : les tours de poules s'appellent « Round #n ».
    fn give_name(&mut self, index: usize) {
        if self.stages[index].kind() != StageKind::PoolSupervisor {
            return;
        }
        let round = self.stages[..=index].iter().filter(|s| s.kind() == StageKind::PoolSupervisor).count();
        let name = crate::i18n::tr("Round #%d").replace("%d", &round.to_string());
        self.stages[index].set_name(&name);
    }

    /// Nom complet affiché dans l'onglet (`GetFullName`).
    pub fn full_name(&self, index: usize) -> String {
        match self.stages.get(index) {
            Some(s) => format!("{}\n{}", crate::i18n::tr(s.kind().class_name()), s.name()),
            None => String::new(),
        }
    }

    // ----------------------------------------------------------------------
    // Navigation

    pub fn set_current(&mut self, index: usize) {
        self.current_stage = index.min(self.stages.len().saturating_sub(1));
    }

    /// `RefreshSensitivity`.
    pub fn sensitivity(&self, ctx: &StageContext) -> Sensitivity {
        let mut s = Sensitivity { next: false, previous: self.current_stage > 0 };
        if !self.stages.is_empty() && self.current_stage + 1 < self.stages.len() {
            s.next = self.stages[self.current_stage].is_over(ctx);
        }
        s
    }

    /// Erreur de la phase courante (`GetError`).
    pub fn current_error(&self, ctx: &StageContext) -> Option<crate::stage::StageError> {
        if self.current_stage + 1 < self.stages.len() {
            self.stages[self.current_stage].error(ctx)
        } else {
            None
        }
    }

    /// Seule la phase d'index `i` ou celle d'avant peut-elle recevoir une
    /// nouvelle phase ? (`on_stage_selected` : insertion impossible entre
    /// deux phases dont l'une est verrouillée.)
    pub fn can_add_after(&self, index: usize) -> bool {
        match (self.stages.get(index), self.stages.get(index + 1)) {
            (Some(a), Some(b)) => !(a.base().locked || b.base().locked),
            _ => true,
        }
    }

    /// Une phase peut-elle être supprimée (`REMOVABLE` et non verrouillée) ?
    pub fn can_remove(&self, index: usize) -> bool {
        self.stages.get(index).map(|s| s.kind().rights().removable && !s.base().locked).unwrap_or(false)
    }

    /// Les phases verrouillées (icône de cadenas dans la formule).
    pub fn lock_flags(&self) -> Vec<bool> {
        self.stages.iter().map(|s| s.base().locked).collect()
    }

    /// `PlugStage` : graine aléatoire de la phase, dérivée de celle de la
    /// phase précédente, qui attribue alors leur rang final à ses éliminés
    /// (`Stage::SetRandSeed`). À appeler avant de charger ou de préparer la phase.
    fn plug_stage(&mut self, ctx: &mut StageContext, index: usize) {
        if index > 0 {
            let previous_seed = self.stages[index - 1].base().rand_seed;
            let id = self.stages[index].base().id;
            self.stages[index].base_mut().rand_seed = stage_rand_seed(id, previous_seed);
            self.give_eliminated_a_final_rank(ctx, index - 1);
        }
        self.stages[index].on_plugged(ctx);
    }

    /// `Stage::GiveEliminatedAFinalRank` : les éliminés d'une phase verrouillée
    /// prennent le rang qu'ils y ont obtenu comme rang final, sauf si la phase
    /// suivante établit son propre classement : seuls les abandons (`A`, rangés
    /// derrière les qualifiés), éliminés (`N`) et exclus (`E`) sont alors classés.
    pub fn give_eliminated_a_final_rank(&self, ctx: &mut StageContext, index: usize) {
        let Some(stage) = self.stages.get(index) else { return };
        if stage.is_input_provider() || !stage.base().locked {
            return;
        }
        let next_has_own_ranking = self.stages.get(index + 1).map(|n| n.has_its_own_ranking()).unwrap_or(false);
        let entries = stage.classification(ctx);
        let qualified = stage.output(ctx).len() as u32;
        let final_rank = AttrId::global("final_rank");
        let mut withdrawal_fix = 0;
        for e in entries {
            let value = if !next_has_own_ranking {
                Some(e.rank)
            } else {
                match e.status {
                    'A' if e.rank <= qualified => {
                        withdrawal_fix += 1;
                        Some(qualified + withdrawal_fix)
                    }
                    'A' | 'N' | 'E' => Some(e.rank),
                    _ => None,
                }
            };
            if let (Some(v), Some(p)) = (value, ctx.players.get_mut(e.player)) {
                p.set_uint(&final_rank, v);
            }
        }
    }

    /// `Stage::RetrieveAttendees` : les qualifiés de la phase précédente
    /// deviennent les participants de la phase `index`, avec leur rang de
    /// départ et le statut « Q ».
    pub fn retrieve_attendees(&mut self, ctx: &mut StageContext, index: usize) {
        let attendees: Vec<PlayerRef> = if index == 0 { Vec::new() } else { self.stages[index - 1].output(ctx) };
        let data_owner = self.data_owner(index);
        let start_id = AttrId::local("stage_start_rank", data_owner);
        let status_id = AttrId::local("status", data_owner);
        let global_status = AttrId::global("global_status");
        for (i, r) in attendees.iter().enumerate() {
            if let Some(p) = ctx.players.get_mut(*r) {
                p.set_uint(&start_id, i as u32 + 1);
                p.set_str(&status_id, "Q");
                p.set_str(&global_status, "Q");
            }
        }
        let n = attendees.len() as u32;
        let stage = &mut self.stages[index];
        stage.set_attendees(ctx, attendees);
        if stage.base().nb_qualified.is_none() && n > 0 {
            stage.base_mut().nb_qualified = Some(n);
        }
    }

    /// `GetPlayerDataOwner` : les attributs locaux d'une phase lui appartiennent.
    pub fn data_owner(&self, index: usize) -> Owner {
        self.stages.get(index).map(|s| s.base().owner()).unwrap_or(Owner::Global)
    }

    /// `on_next_stage_toolbutton_clicked` (sans la boîte de dialogue du
    /// barrage). Verrouille la phase courante et prépare la suivante.
    /// Renvoie le dépassement de quota de la phase verrouillée : s'il est
    /// non nul, l'interface propose d'ajouter un barrage (`add_barrage`)
    /// **avant** d'appeler `enter_next`.
    pub fn lock_current(&mut self, ctx: &mut StageContext) -> u32 {
        let i = self.current_stage;
        if i >= self.stages.len() {
            return 0;
        }
        self.stages[i].lock(ctx);
        self.apply_renumbering(ctx);
        self.refresh_previous();
        self.stages[i].base().quota_exceedance
    }

    /// Insère un barrage après la phase courante (`CreateStage ("Barrage")`).
    pub fn add_barrage(&mut self, ctx: &mut StageContext) -> usize {
        let after = self.current_stage;
        self.add_kind(ctx, StageKind::Barrage, Some(after))
    }

    /// Entre dans la phase suivante : graine, participants, préparation.
    pub fn enter_next(&mut self, ctx: &mut StageContext) {
        let next = self.current_stage + 1;
        if next >= self.stages.len() {
            return;
        }
        self.plug_stage(ctx, next);
        self.retrieve_attendees(ctx, next);
        self.prepare_divisions(next);
        self.apply_youth_training(next);
        self.stages[next].garnish(ctx);
        self.set_current(next);
        self.refresh_general_classification(ctx);
    }

    /// Poules de division : la répartition du tour de division précédent
    /// (montées et descentes), sinon la répartition de départ.
    fn prepare_divisions(&mut self, index: usize) {
        use crate::rounds::divisions::DivisionRound;
        if !self.stages[index].as_any().is::<DivisionRound>() {
            return;
        }
        let previous = self.stages[..index]
            .iter()
            .rev()
            .find_map(|s| s.as_any().downcast_ref::<DivisionRound>())
            .map(|d| d.assignment().clone());
        if let Some(round) = self.stages[index].as_any_mut().downcast_mut::<DivisionRound>() {
            round.prepare(previous.as_ref());
        }
    }

    /// Entraînement des jeunes (extension) : une phase de poules ou de
    /// tableaux placée après la création des groupes travaille groupe par
    /// groupe (poules de chaque groupe, sortie croisée, places tirées).
    fn apply_youth_training(&mut self, index: usize) {
        use crate::rounds::youth::{PoolMode, YouthTraining};
        use crate::rounds::divisions::DivisionRound;
        // La phase de groupes la plus proche : entraînement des jeunes ou poules de division.
        let grouping = self.stages[..index].iter().rev().find(|s| s.as_any().is::<YouthTraining>() || s.as_any().is::<DivisionRound>());
        let youth = grouping
            .and_then(|s| s.as_any().downcast_ref::<YouthTraining>())
            .map(|y| (y.settings.clone(), y.locked_layout()));
        let divisions = grouping.and_then(|s| s.as_any().downcast_ref::<DivisionRound>()).map(|d| d.settings.clone());
        let stage = &mut self.stages[index];
        if let Some(allocator) = stage.as_any().downcast_ref::<crate::rounds::pool::PoolAllocatorStage>() {
            let mut round = allocator.round();
            match (&youth, &divisions) {
                (Some((settings, layout)), _) => {
                    round.youth_groups = Some(layout.iter().map(|l| l.nb_pools).collect());
                    round.cross_output = true;
                    round.group_prefix = None;
                    round.set_youth_seeding(settings.pool_mode == PoolMode::Snake, &settings.swapping);
                }
                // Poules de division : les poules de chaque division, classement D1 d'abord.
                (None, Some(settings)) => {
                    round.youth_groups = Some(vec![settings.pools.max(1); settings.divisions.max(1) as usize]);
                    round.cross_output = true;
                    round.group_prefix = Some("D".into());
                    round.set_youth_seeding(true, &settings.swapping);
                }
                (None, None) => {
                    round.youth_groups = None;
                    round.cross_output = false;
                    round.group_prefix = None;
                }
            }
        } else if let Some(table) = stage.as_any_mut().downcast_mut::<crate::rounds::table::supervisor::TableSupervisor>() {
            if let Some((settings, _)) = &youth {
                table.fenced_places = settings.fenced_places.value();
            }
        }
    }

    /// `GeneralClassification::GetCurrentClassification` : le classement
    /// général est recalculé à partir des présents du pointage et du rang de
    /// chaque qualifié dans la phase précédente. À rappeler après toute
    /// modification des phases qui le précèdent.
    pub fn refresh_general_classification(&mut self, ctx: &mut StageContext) {
        let presents: Vec<PlayerRef> = match self.checkin() {
            Some(c) => c.presents().to_vec(),
            None => return,
        };
        for i in 1..self.stages.len() {
            if !self.stages[i].as_any().is::<GeneralClassificationStage>() || i > self.current_stage {
                continue;
            }
            // Restaurer les rangs bruts des elimines avant de retrancher les
            // exclus/exportes : les rangs finaux du calcul precedent sont
            // deja decales et ne doivent pas etre decales une seconde fois.
            for previous in 0..i {
                if self.stages[previous].kind() != StageKind::GeneralClassification {
                    self.give_eliminated_a_final_rank(ctx, previous);
                }
            }
            let previous_owner = self.stages[i - 1].base().owner();
            let rank = AttrId::local("rank", previous_owner);
            let ranks: HashMap<PlayerRef, u32> = self.stages[i]
                .base()
                .attendees
                .iter()
                .filter_map(|r| ctx.players.get(*r).and_then(|p| p.get(&rank)).map(|v| (*r, v.as_uint())))
                .collect();
            if let Some(gc) = self.stages[i].as_any_mut().downcast_mut::<GeneralClassificationStage>() {
                gc.set_context(presents.clone(), ranks);
                gc.refresh(ctx);
            }
        }
    }

    /// `on_previous_stage_toolbutton_clicked` : annule la phase courante et
    /// déverrouille la précédente. Un barrage est supprimé.
    pub fn go_back(&mut self, ctx: &mut StageContext) {
        let i = self.current_stage;
        if i == 0 || i >= self.stages.len() {
            return;
        }
        if self.stages[i].kind() == StageKind::Barrage {
            self.remove_stage(ctx, i);
        } else {
            self.stages[i].reset(ctx);
            self.set_current(i - 1);
            let c = self.current_stage;
            self.stages[c].unlock(ctx);
        }
        self.refresh_previous();
    }

    /// `RemoveStage`.
    pub fn remove_stage(&mut self, ctx: &mut StageContext, index: usize) {
        if index >= self.stages.len() {
            return;
        }
        if self.current_stage > 0 && self.current_stage == index {
            self.stages[index].reset(ctx);
            self.set_current(self.current_stage - 1);
            let c = self.current_stage;
            self.stages[c].unlock(ctx);
        }
        self.stages.remove(index);
        if self.current_stage >= self.stages.len() {
            self.current_stage = self.stages.len().saturating_sub(1);
        }
        self.after_structure_change();
    }

    /// `on_stage_removed` : supprime une phase supprimable et son fournisseur
    /// d'entrée. Renvoie vrai si quelque chose a été supprimé.
    pub fn remove_user_stage(&mut self, ctx: &mut StageContext, index: usize) -> bool {
        if !self.can_remove(index) {
            return false;
        }
        let provider = self.stages[index].input_provider().is_some() && index > 0 && self.stages[index - 1].is_input_provider();
        self.remove_stage(ctx, index);
        if provider {
            self.remove_stage(ctx, index - 1);
        }
        true
    }

    /// Bloc d'une phase dans la formule : la phase et son fournisseur
    /// d'entrée (composition + tour de poules).
    fn block_of(&self, index: usize) -> Option<std::ops::RangeInclusive<usize>> {
        let stage = self.stages.get(index)?;
        if stage.is_input_provider() && index + 1 < self.stages.len() {
            return Some(index..=index + 1);
        }
        if stage.input_provider().is_some() && index > 0 && self.stages[index - 1].is_input_provider() {
            return Some(index - 1..=index);
        }
        Some(index..=index)
    }

    /// Un bloc peut changer de place : phase supprimable, pas encore jouée.
    fn block_movable(&self, block: &std::ops::RangeInclusive<usize>) -> bool {
        *block.start() > self.current_stage && self.can_remove(*block.end()) && block.clone().all(|i| !self.stages[i].base().locked)
    }

    /// Blocs à échanger pour monter (`up`) ou descendre la phase `index`.
    fn move_blocks(&self, index: usize, up: bool) -> Option<(std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>)> {
        let mine = self.block_of(index)?;
        let other = if up {
            self.block_of(mine.start().checked_sub(1)?)?
        } else {
            self.block_of(mine.end() + 1)?
        };
        (self.block_movable(&mine) && self.block_movable(&other)).then_some((mine, other))
    }

    /// La phase peut-elle monter (`up`) ou descendre d'un cran dans la formule ?
    pub fn can_move(&self, index: usize, up: bool) -> bool {
        self.move_blocks(index, up).is_some()
    }

    /// Échange la phase (avec sa composition de poules) et sa voisine.
    /// Renvoie le nouvel index de la phase.
    pub fn move_user_stage(&mut self, index: usize, up: bool) -> Option<usize> {
        let (mine, other) = self.move_blocks(index, up)?;
        let (first, second) = if up { (other, mine.clone()) } else { (mine.clone(), other) };
        let first_len = first.end() - first.start() + 1;
        self.stages[*first.start()..=*second.end()].rotate_left(first_len);
        self.after_structure_change();
        for i in 0..self.stages.len() {
            self.give_name(i);
        }
        let offset = index - mine.start();
        Some(if up { *first.start() + offset } else { *first.start() + (second.end() - second.start() + 1) + offset })
    }

    /// Applique la renumérotation faite par le pointage aux autres phases.
    pub fn apply_renumbering(&mut self, ctx: &mut StageContext) {
        let _ = ctx;
        let map = self.checkin_mut().and_then(|c| c.take_renumbering());
        if let Some(map) = map {
            self.remap_players(&map);
        }
    }

    pub fn remap_players(&mut self, map: &HashMap<PlayerRef, PlayerRef>) {
        for s in self.stages.iter_mut().skip(1) {
            s.remap_players(map);
        }
    }

    // ----------------------------------------------------------------------
    // Fichiers

    /// `Schedule::Load` (partie phases). `phases` est l'élément `<Phases>`
    /// s'il existe ; `root` la racine de la compétition. Le pointage reçoit
    /// d'abord les joueurs (`LoadPeoples`).
    ///
    /// Les phases antérieures à la phase en cours sont verrouillées, comme
    /// à l'affichage dans le C++.
    pub fn load(&mut self, ctx: &mut StageContext, root: roxmltree::Node, extra: &[String]) -> Result<(), Error> {
        let phases = crate::xml::child(root, "Phases");
        let current_stage_index: i64 = phases
            .and_then(|p| p.attribute("PhaseEnCours"))
            .map(crate::attribute::parse_leading_int)
            .unwrap_or(-1);
        let display_all = current_stage_index < 0;
        self.loading = true;

        // Pointage ajouté s'il manque
        let has_checkin = phases.map(|p| crate::xml::child(p, "Pointage").is_some()).unwrap_or(false);
        let mut nb_stage = 0usize;
        if !has_checkin {
            self.add_kind(ctx, StageKind::Checkin, None);
            self.load_peoples(ctx, root, extra);
            self.load_stage(ctx, 0, None, 1, current_stage_index, display_all)?;
            nb_stage = 1;
        }

        if let Some(phases) = phases {
            let mut first = true;
            for node in phases.children().filter(|n| n.is_element()) {
                let Some(kind) = StageKind::from_xml_name(node.tag_name().name()) else { continue };
                let index = self.add_kind(ctx, kind, None);
                if first {
                    if nb_stage == 0 {
                        self.load_peoples(ctx, root, extra);
                    } else {
                        // pointage injecté : verrouillé
                        self.stages[0].lock(ctx);
                    }
                    first = false;
                }
                // fournisseur d'entrée éventuel : chargé avant la phase
                if self.stages[index].input_provider().is_some() && index > 0 && self.stages[index - 1].is_input_provider() {
                    nb_stage += 1;
                    self.load_stage(ctx, index - 1, Some(node), nb_stage, current_stage_index, display_all)?;
                }
                nb_stage += 1;
                self.load_stage(ctx, index, Some(node), nb_stage, current_stage_index, display_all)?;
            }
        }

        // Phases complémentaires pour les fichiers d'autres logiciels
        let mut current = current_stage_index;
        if nb_stage > 0 && !self.stages.last().map(|s| s.as_any().is::<GeneralClassificationStage>()).unwrap_or(false) {
            current = nb_stage as i64 - 1;
            if nb_stage == 1 {
                self.add_kind(ctx, StageKind::PoolSupervisor, None);
                self.add_kind(ctx, StageKind::TableSupervisor, None);
            }
            self.add_kind(ctx, StageKind::GeneralClassification, None);
        }

        self.set_current(current.max(0) as usize);
        Ok(())
    }

    fn load_peoples(&mut self, ctx: &mut StageContext, root: roxmltree::Node, extra: &[String]) {
        use crate::player::PlayerClass;
        if let Some(checkin) = self.checkin_mut() {
            checkin.load_list(ctx, root, PlayerClass::Team, extra);
            checkin.load_list(ctx, root, PlayerClass::Fencer, extra);
        }
    }

    /// `LoadStage` (version à 4 paramètres).
    fn load_stage(
        &mut self,
        ctx: &mut StageContext,
        index: usize,
        node: Option<roxmltree::Node>,
        nb_stage: usize,
        current_stage_index: i64,
        display_all: bool,
    ) -> Result<(), Error> {
        let shown = display_all || (nb_stage as i64 - 1) <= current_stage_index;
        if shown {
            self.plug_stage(ctx, index);
        }
        if let Some(node) = node {
            self.stages[index].load(ctx, node)?;
            // Le classement général, l'entraînement des jeunes, le point de
            // scission et le lobby n'enregistrent pas leurs participants : ce
            // sont les qualifiés de la phase d'avant.
            let derived = self.stages[index].as_any().is::<GeneralClassificationStage>()
                || self.stages[index].as_any().is::<crate::rounds::youth::YouthTraining>()
                || self.stages[index].as_any().is::<crate::rounds::splitting::Splitting>()
                || self.stages[index].as_any().is::<crate::rounds::divisions::DivisionRound>()
                || self.stages[index].as_any().is::<crate::rounds::lobby::Lobby>();
            if derived && index > 0 && self.stages[index - 1].base().locked {
                self.retrieve_attendees(ctx, index);
            }
        }
        if shown && (display_all || (nb_stage as i64 - 1) < current_stage_index) {
            self.stages[index].lock(ctx);
        }
        Ok(())
    }

    /// `OnLoadingCompleted` : puis application de la renumérotation différée
    /// du pointage à toutes les phases.
    pub fn on_loading_completed(&mut self, ctx: &mut StageContext) {
        let map = self.checkin_mut().and_then(|c| c.apply_pending_renumbering(ctx));
        if let Some(map) = map {
            self.remap_players(&map);
        }
        for s in self.stages.iter_mut() {
            s.on_loading_completed(ctx);
        }
        if let Some(c) = self.checkin_mut() {
            c.set_loading(false);
        }
        self.loading = false;
        self.refresh_general_classification(ctx);
    }

    /// `Schedule::Save` : `<Phases PhaseEnCours=…>` et chaque phase.
    pub fn save(&self, ctx: &StageContext, w: &mut XmlWriter) {
        w.start("Phases");
        w.attr_fmt("PhaseEnCours", self.current_stage);
        for s in &self.stages {
            s.save(ctx, w);
        }
        w.end();
    }

    /// Export HTML de toutes les phases (`DumpToHTML`).
    pub fn dump_to_html(&self, ctx: &StageContext, html: &mut String) {
        for s in &self.stages {
            s.dump_to_html(ctx, html);
        }
    }
}
