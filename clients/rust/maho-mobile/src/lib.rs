pub mod android;
pub mod bridge;
pub mod ios;
pub mod keyboard;
pub mod lifecycle;
pub mod power;
pub mod storage;
pub mod touch;

pub use android::{AndroidAudioTrackPlayer, AndroidMediaCodecConfig, AndroidMediaCodecDecoder};
pub use bridge::{
    maho_mobile_create, maho_mobile_destroy, maho_mobile_send_touch, MahoMobileStatus,
};
pub use ios::{IosAudioEnginePlayer, IosVideoToolboxConfig, IosVideoToolboxDecoder};
pub use keyboard::{ImeComposition, MobileAccessoryKey, MobileModifierBar};
pub use lifecycle::{
    AppLifecycleState, DeviceOrientation, MobileLifecycleManager, NetworkInterfaceType,
};
pub use power::{PowerBudgetConfig, PowerPolicyManager, ThermalState};
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use storage::IosKeychainStorage;
pub use storage::{
    MobilePairingStore, MockSecureStorage, SecureStorageBackend, SecureStorageError,
};
pub use touch::{TouchGestureHandler, TouchMode, TouchPhase, TouchPoint, ViewportState};
