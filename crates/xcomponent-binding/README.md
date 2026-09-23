# XComponent callback routing for multiple native windows

This is `ohos-xcomponent-binding` 0.4.1 with its `multi_mode` callback registry
keyed by the `OH_NativeXComponent` pointer. The published 0.4.1 registry uses
`OH_NativeXComponent_GetXComponentId`, which returns an empty string for the
native ArkUI XComponents created before their surfaces are mounted. A second
window then replaces the first window's callback set.

The native component pointer identifies the same instance at registration,
callback dispatch, and disposal. Keep this local patch until the upstream
binding provides per-instance callback routing for native ArkUI components.

Original crate: https://crates.io/crates/ohos-xcomponent-binding/0.4.1
License: MIT OR Apache-2.0 (as declared by the original crate).
