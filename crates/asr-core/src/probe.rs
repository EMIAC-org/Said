//! Vulkan capability + device selection (worker-only, `vulkan` feature).
//!
//! Runs inside the isolated GPU worker before it touches whisper. It answers two
//! questions crash-safely (any loader/driver problem yields `None`, never a
//! process abort):
//!   1. Is there a usable Vulkan GPU at all?
//!   2. If several, which one should whisper use?
//!
//! ## Index consistency with ggml
//! whisper's `gpu_device` indexes ggml's *filtered* device list — ggml keeps
//! discrete + integrated GPUs supporting Vulkan ≥ 1.2, in raw enumeration order.
//! We reproduce exactly that filter over `ash`'s `enumerate_physical_devices()`,
//! so the position of our pick in the filtered list equals the ggml index we
//! hand to whisper. (ggml additionally de-dupes GPUs exposed by two drivers via
//! device UUID; that only occurs with multiple ICDs for one card — rare on
//! Windows — and is the single documented edge case where indices could differ.)
//!
//! Selection policy: prefer a **discrete** GPU; break ties by device-local VRAM.

use std::ffi::CStr;

use ash::vk;

use crate::ipc::{BackendKind, DeviceInfo};

/// Pick the best usable Vulkan GPU, or `None` if there is no loader / no
/// suitable device. Never panics or aborts.
#[must_use]
pub fn select_best_gpu() -> Option<DeviceInfo> {
    // `Entry::load()` dynamically loads the Vulkan loader; a missing/absent
    // vulkan-1.dll returns Err here rather than killing the process.
    let entry = unsafe { ash::Entry::load() }.ok()?;

    // Request only 1.0 for instance creation so this never trips
    // ERROR_INCOMPATIBLE_DRIVER; per-device 1.2 support is checked below.
    let app_info = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 0, 0));
    let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
    let instance = unsafe { entry.create_instance(&create_info, None) }.ok()?;

    let chosen = select_from_instance(&instance);

    // Always tear down the throwaway instance.
    unsafe { instance.destroy_instance(None) };
    chosen
}

fn select_from_instance(instance: &ash::Instance) -> Option<DeviceInfo> {
    let physical_devices = unsafe { instance.enumerate_physical_devices() }.ok()?;

    // ggml's `gpu_device` index: position among kept (discrete|integrated, ≥1.2)
    // devices in enumeration order.
    let mut ggml_index: i32 = -1;
    let mut best: Option<(DeviceInfo, DeviceScore)> = None;

    for pd in physical_devices {
        let props = unsafe { instance.get_physical_device_properties(pd) };

        let discrete = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => true,
            vk::PhysicalDeviceType::INTEGRATED_GPU => false,
            _ => continue, // ggml keeps only discrete/integrated
        };
        // ggml requires Vulkan 1.2+ on the device.
        if props.api_version < vk::make_api_version(0, 1, 2, 0) {
            continue;
        }

        ggml_index += 1;

        let info = DeviceInfo {
            backend: BackendKind::Vulkan,
            index: ggml_index,
            name: device_name(&props),
            vram_mb: device_local_vram_mb(instance, pd),
            discrete,
        };
        let score = DeviceScore {
            discrete,
            vram_mb: info.vram_mb,
        };

        if best.as_ref().is_none_or(|(_, s)| score > *s) {
            best = Some((info, score));
        }
    }

    best.map(|(info, _)| info)
}

/// Ranking key: discrete beats integrated; then more device-local VRAM wins.
#[derive(PartialEq, Eq)]
struct DeviceScore {
    discrete: bool,
    vram_mb: u64,
}

impl PartialOrd for DeviceScore {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DeviceScore {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.discrete
            .cmp(&other.discrete)
            .then(self.vram_mb.cmp(&other.vram_mb))
    }
}

fn device_name(props: &vk::PhysicalDeviceProperties) -> String {
    // `device_name` is a NUL-terminated fixed [c_char; 256].
    let ptr = props.device_name.as_ptr();
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_string_lossy().into_owned()
}

fn device_local_vram_mb(instance: &ash::Instance, pd: vk::PhysicalDevice) -> u64 {
    let mem = unsafe { instance.get_physical_device_memory_properties(pd) };
    let mut bytes: u64 = 0;
    for i in 0..mem.memory_heap_count as usize {
        let heap = mem.memory_heaps[i];
        if heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL) {
            bytes = bytes.saturating_add(heap.size);
        }
    }
    bytes / (1024 * 1024)
}
