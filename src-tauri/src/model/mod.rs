pub mod asset;
pub mod asset_record;
pub mod game_config;
pub mod project;

// Convenience re-exports forming the model's public surface. Not every symbol is
// used internally yet (later milestones will), so allow the unused-import lint.
#[allow(unused_imports)]
pub use asset::{AssetDescriptor, AssetKind, Format, Insets, Production};
#[allow(unused_imports)]
pub use asset_record::{AssetRecord, PromptState, Variation, VariationStatus};
#[allow(unused_imports)]
pub use game_config::{BuyBonusMode, GameConfig, SymbolDef, SymbolRole, WinType};
#[allow(unused_imports)]
pub use project::{Project, ProjectSummary};
