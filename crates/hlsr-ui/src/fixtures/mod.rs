//! Сцены экранов на фикстурах (И2, решение 16): каталог `tools/snap/screens` (тот же, что снимает
//! эталон `tools/reference/snap-screens.js`), `FixtureServices` — главный процесс на ответах сцены
//! с журналом вызовов. Окно сцены, шаги и песочницу платформы даёт вид (`hlsr-snap`).

pub mod journal;
pub mod scene;
pub mod services;

pub use scene::{Scene, Variant, VariantFilter, scene_ids, screens_049_dir, screens_dir};
pub use services::FixtureServices;
