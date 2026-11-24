//! printdex-core integration tests, built as one binary: each test file
//! used to link the whole engine (rustic, OpenDAL, sqlx) on its own (#104).

mod backup_config;
mod backup_data;
mod backup_models;
mod backup_repo;
mod backup_restore;
mod backup_scheduler;
mod chaos;
mod engine;
mod file_ops;
mod identity;
mod monitor;
mod open;
mod prints;
mod sidecars;
mod supervise;
mod sync;
mod thumbs;
