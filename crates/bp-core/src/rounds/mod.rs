//! Phases concrètes (port de `supervisor/rounds/`).

pub mod checkin;
pub mod classification;
pub mod pool;
pub mod table;
pub mod barrage;
pub mod splitting;
pub mod youth;
pub mod divisions;
pub mod lobby;

use crate::stage::{Stage, StageKind};

/// Crée une phase vide d'un type donné (`Stage::CreateInstance`).
pub fn create_stage(kind: StageKind, id: u32) -> Box<dyn Stage> {
    match kind {
        StageKind::Checkin => checkin::new_stage(kind, id),
        StageKind::PoolAllocator | StageKind::PoolSupervisor => pool::new_stage(kind, id),
        StageKind::TableSupervisor => table::new_stage(kind, id),
        StageKind::Barrage => barrage::new_stage(kind, id),
        StageKind::Splitting => splitting::new_stage(kind, id),
        StageKind::GeneralClassification => classification::new_stage(kind, id),
        StageKind::YouthTraining => youth::new_stage(kind, id),
        StageKind::DivisionRound => divisions::new_stage(kind, id),
        StageKind::Lobby => lobby::new_stage(kind, id),
    }
}
