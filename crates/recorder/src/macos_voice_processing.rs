use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use super::{RecCmd, fan_out_mono};

type OSStatus = i32;
type AudioComponent = *mut c_void;
type AudioUnit = *mut c_void;
type AudioUnitRenderActionFlags = u32;

const NO_ERR: OSStatus = 0;
const INPUT_BUS: u32 = 1;
const OUTPUT_BUS: u32 = 0;
const VOICE_PROCESSING_SAMPLE_RATE: u32 = 48_000;

const K_AUDIO_UNIT_TYPE_OUTPUT: u32 = fourcc(*b"auou");
const K_AUDIO_UNIT_SUB_TYPE_VOICE_PROCESSING_IO: u32 = fourcc(*b"vpio");
const K_AUDIO_UNIT_MANUFACTURER_APPLE: u32 = fourcc(*b"appl");
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = fourcc(*b"lpcm");

const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1 << 0;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;

const K_AUDIO_UNIT_SCOPE_GLOBAL: u32 = 0;
const K_AUDIO_UNIT_SCOPE_INPUT: u32 = 1;
const K_AUDIO_UNIT_SCOPE_OUTPUT: u32 = 2;

const K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;
const K_AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO: u32 = 2003;
const K_AUDIO_OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK: u32 = 2005;
const K_AU_VOICE_IO_PROPERTY_BYPASS_VOICE_PROCESSING: u32 = 2100;
const K_AU_VOICE_IO_PROPERTY_VOICE_PROCESSING_ENABLE_AGC: u32 = 2101;
const K_AU_VOICE_IO_PROPERTY_OTHER_AUDIO_DUCKING_CONFIGURATION: u32 = 2108;
const K_AU_VOICE_IO_OTHER_AUDIO_DUCKING_LEVEL_MIN: u32 = 10;
const IDLE_UNIT_TTL_MS: u64 = 750;

static WORKER_TX: OnceLock<mpsc::Sender<WorkerCmd>> = OnceLock::new();
static DUCKING_CONFIG_WARNING: OnceLock<()> = OnceLock::new();

const fn fourcc(bytes: [u8; 4]) -> u32 {
    ((bytes[0] as u32) << 24)
        | ((bytes[1] as u32) << 16)
        | ((bytes[2] as u32) << 8)
        | bytes[3] as u32
}

#[repr(C)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AURenderCallbackStruct {
    input_proc: Option<AURenderCallback>,
    input_proc_ref_con: *mut c_void,
}

#[repr(C)]
struct AUVoiceIOOtherAudioDuckingConfiguration {
    enable_advanced_ducking: u8,
    ducking_level: u32,
}

type AURenderCallback = unsafe extern "C" fn(
    *mut c_void,
    *mut AudioUnitRenderActionFlags,
    *const c_void,
    u32,
    u32,
    *mut AudioBufferList,
) -> OSStatus;

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        in_component: AudioComponent,
        in_desc: *const AudioComponentDescription,
    ) -> AudioComponent;

    fn AudioComponentInstanceNew(
        in_component: AudioComponent,
        out_instance: *mut AudioUnit,
    ) -> OSStatus;

    fn AudioComponentInstanceDispose(in_instance: AudioUnit) -> OSStatus;

    fn AudioUnitSetProperty(
        in_unit: AudioUnit,
        in_id: u32,
        in_scope: u32,
        in_element: u32,
        in_data: *const c_void,
        in_data_size: u32,
    ) -> OSStatus;

    fn AudioUnitInitialize(in_unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(in_unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStart(in_unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStop(in_unit: AudioUnit) -> OSStatus;

    fn AudioUnitRender(
        in_unit: AudioUnit,
        io_action_flags: *mut AudioUnitRenderActionFlags,
        in_time_stamp: *const c_void,
        in_output_bus_number: u32,
        in_number_frames: u32,
        io_data: *mut AudioBufferList,
    ) -> OSStatus;
}

#[derive(Clone)]
struct ActiveCapture {
    frames: Arc<Mutex<Vec<f32>>>,
    chunk_tx: mpsc::SyncSender<Vec<f32>>,
    level_tx: mpsc::SyncSender<f32>,
}

struct CallbackState {
    audio_unit: AudioUnit,
    active: Mutex<Option<ActiveCapture>>,
}

struct PreparedUnit {
    unit: AudioUnit,
    state_ptr: *mut CallbackState,
}

enum WorkerCmd {
    Warm,
    Begin {
        frames: Arc<Mutex<Vec<f32>>>,
        ready_tx: mpsc::Sender<Result<u32, String>>,
        chunk_tx: mpsc::SyncSender<Vec<f32>>,
        level_tx: mpsc::SyncSender<f32>,
    },
    Stop {
        reply: Option<mpsc::Sender<(Vec<f32>, u32)>>,
    },
}

pub(super) fn prewarm() {
    let _ = worker_sender().send(WorkerCmd::Warm);
}

pub(super) fn spawn_capture_thread(
    frames: Arc<Mutex<Vec<f32>>>,
    cmd_rx: mpsc::Receiver<RecCmd>,
    ready_tx: mpsc::Sender<Result<u32, String>>,
    chunk_tx: mpsc::SyncSender<Vec<f32>>,
    level_tx: mpsc::SyncSender<f32>,
) {
    let worker_tx = worker_sender();
    std::thread::spawn(move || {
        if worker_tx
            .send(WorkerCmd::Begin {
                frames,
                ready_tx,
                chunk_tx,
                level_tx,
            })
            .is_err()
        {
            return;
        }

        let reply = match cmd_rx.recv() {
            Ok(RecCmd::Stop(reply)) => Some(reply),
            Err(_) => None,
        };
        let _ = worker_tx.send(WorkerCmd::Stop { reply });
    });
}

fn worker_sender() -> mpsc::Sender<WorkerCmd> {
    if let Some(tx) = WORKER_TX.get() {
        return tx.clone();
    }

    let (tx, rx) = mpsc::channel::<WorkerCmd>();
    if WORKER_TX.set(tx.clone()).is_ok() {
        std::thread::spawn(move || worker_loop(rx));
        tx
    } else {
        WORKER_TX
            .get()
            .expect("VoiceProcessingIO worker sender should exist")
            .clone()
    }
}

fn worker_loop(rx: mpsc::Receiver<WorkerCmd>) {
    let mut prepared: Option<PreparedUnit> = None;
    let mut recording = false;

    loop {
        let cmd = match recv_worker_cmd(&rx, prepared.is_some() && !recording) {
            Ok(cmd) => cmd,
            Err(WorkerIdle::TimedOut) => {
                if let Some(unit) = prepared.take() {
                    unsafe {
                        teardown_prepared(unit);
                    }
                    println!("[rec] released idle Apple VoiceProcessingIO");
                }
                continue;
            }
            Err(WorkerIdle::Disconnected) => break,
        };

        match cmd {
            WorkerCmd::Warm => {
                if recording || prepared.is_some() {
                    continue;
                }

                match unsafe { prepare_voice_processing_unit() } {
                    Ok(unit) => {
                        println!(
                            "[rec] briefly warmed Apple VoiceProcessingIO at {VOICE_PROCESSING_SAMPLE_RATE}Hz float mono"
                        );
                        prepared = Some(unit);
                    }
                    Err(e) => {
                        eprintln!("[rec] Apple VoiceProcessingIO warm-up failed: {e}");
                    }
                }
            }
            WorkerCmd::Begin {
                frames,
                ready_tx,
                chunk_tx,
                level_tx,
            } => {
                if prepared.is_none() {
                    match unsafe { prepare_voice_processing_unit() } {
                        Ok(unit) => {
                            println!(
                                "[rec] opened Apple VoiceProcessingIO at {VOICE_PROCESSING_SAMPLE_RATE}Hz float mono"
                            );
                            prepared = Some(unit);
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            continue;
                        }
                    }
                }

                let Some(unit) = prepared.as_ref() else {
                    let _ = ready_tx.send(Err("Apple VoiceProcessingIO unavailable".into()));
                    continue;
                };

                match unsafe {
                    begin_capture(
                        unit,
                        ActiveCapture {
                            frames,
                            chunk_tx,
                            level_tx,
                        },
                    )
                } {
                    Ok(start_ms) => {
                        recording = true;
                        let _ = ready_tx.send(Ok(VOICE_PROCESSING_SAMPLE_RATE));
                        println!("[rec] started Apple VoiceProcessingIO in {start_ms}ms");
                    }
                    Err(e) => {
                        recording = false;
                        let _ = ready_tx.send(Err(e));
                    }
                }
            }
            WorkerCmd::Stop { reply } => {
                if let Some(unit) = prepared.as_ref() {
                    unsafe {
                        stop_capture(unit, reply);
                    }
                } else if let Some(reply) = reply {
                    let _ = reply.send((Vec::new(), VOICE_PROCESSING_SAMPLE_RATE));
                }
                recording = false;
            }
        }
    }

    if let Some(unit) = prepared {
        unsafe {
            teardown_prepared(unit);
        }
    }
}

enum WorkerIdle {
    TimedOut,
    Disconnected,
}

fn recv_worker_cmd(
    rx: &mpsc::Receiver<WorkerCmd>,
    can_release_idle_unit: bool,
) -> Result<WorkerCmd, WorkerIdle> {
    if can_release_idle_unit {
        match rx.recv_timeout(Duration::from_millis(IDLE_UNIT_TTL_MS)) {
            Ok(cmd) => Ok(cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(WorkerIdle::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerIdle::Disconnected),
        }
    } else {
        rx.recv().map_err(|_| WorkerIdle::Disconnected)
    }
}

unsafe fn prepare_voice_processing_unit() -> Result<PreparedUnit, String> {
    let mut unit: AudioUnit = ptr::null_mut();
    let mut state_ptr: *mut CallbackState = ptr::null_mut();

    let result = unsafe { build_voice_processing_unit(&mut unit, &mut state_ptr) }
        .and_then(|()| check("AudioUnitInitialize", unsafe { AudioUnitInitialize(unit) }));
    if let Err(e) = result {
        unsafe {
            dispose_unit(unit, state_ptr);
        }
        return Err(e);
    }

    Ok(PreparedUnit { unit, state_ptr })
}

unsafe fn build_voice_processing_unit(
    unit: *mut AudioUnit,
    state_ptr: *mut *mut CallbackState,
) -> Result<(), String> {
    let desc = AudioComponentDescription {
        component_type: K_AUDIO_UNIT_TYPE_OUTPUT,
        component_sub_type: K_AUDIO_UNIT_SUB_TYPE_VOICE_PROCESSING_IO,
        component_manufacturer: K_AUDIO_UNIT_MANUFACTURER_APPLE,
        component_flags: 0,
        component_flags_mask: 0,
    };

    let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &desc) };
    if component.is_null() {
        return Err("VoiceProcessingIO component not found".into());
    }

    check("AudioComponentInstanceNew", unsafe {
        AudioComponentInstanceNew(component, unit)
    })?;
    let audio_unit = unsafe { *unit };

    let one: u32 = 1;
    let zero: u32 = 0;
    check("enable VoiceProcessingIO input", unsafe {
        AudioUnitSetProperty(
            audio_unit,
            K_AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
            K_AUDIO_UNIT_SCOPE_INPUT,
            INPUT_BUS,
            (&one as *const u32).cast::<c_void>(),
            byte_size_of::<u32>(),
        )
    })?;

    best_effort_set_u32(
        audio_unit,
        "disable VoiceProcessingIO output",
        K_AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
        K_AUDIO_UNIT_SCOPE_OUTPUT,
        OUTPUT_BUS,
        zero,
    );

    let asbd = float_mono_asbd();
    check("set VoiceProcessingIO input stream format", unsafe {
        AudioUnitSetProperty(
            audio_unit,
            K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
            K_AUDIO_UNIT_SCOPE_OUTPUT,
            INPUT_BUS,
            (&asbd as *const AudioStreamBasicDescription).cast::<c_void>(),
            byte_size_of::<AudioStreamBasicDescription>(),
        )
    })?;

    best_effort_set_stream_format(
        audio_unit,
        "set VoiceProcessingIO output stream format",
        K_AUDIO_UNIT_SCOPE_INPUT,
        OUTPUT_BUS,
        &asbd,
    );

    best_effort_set_u32(
        audio_unit,
        "enable VoiceProcessingIO AGC",
        K_AU_VOICE_IO_PROPERTY_VOICE_PROCESSING_ENABLE_AGC,
        K_AUDIO_UNIT_SCOPE_GLOBAL,
        INPUT_BUS,
        one,
    );
    best_effort_set_u32(
        audio_unit,
        "ensure VoiceProcessingIO processing is not bypassed",
        K_AU_VOICE_IO_PROPERTY_BYPASS_VOICE_PROCESSING,
        K_AUDIO_UNIT_SCOPE_GLOBAL,
        INPUT_BUS,
        zero,
    );
    best_effort_set_minimum_ducking(audio_unit);

    let state = Box::new(CallbackState {
        audio_unit,
        active: Mutex::new(None),
    });
    let raw_state = Box::into_raw(state);
    unsafe {
        *state_ptr = raw_state;
    }

    let callback = AURenderCallbackStruct {
        input_proc: Some(input_callback),
        input_proc_ref_con: raw_state.cast::<c_void>(),
    };
    check("set VoiceProcessingIO input callback", unsafe {
        AudioUnitSetProperty(
            audio_unit,
            K_AUDIO_OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK,
            K_AUDIO_UNIT_SCOPE_GLOBAL,
            INPUT_BUS,
            (&callback as *const AURenderCallbackStruct).cast::<c_void>(),
            byte_size_of::<AURenderCallbackStruct>(),
        )
    })
}

unsafe extern "C" fn input_callback(
    ref_con: *mut c_void,
    io_action_flags: *mut AudioUnitRenderActionFlags,
    in_time_stamp: *const c_void,
    _in_bus_number: u32,
    in_number_frames: u32,
    _io_data: *mut AudioBufferList,
) -> OSStatus {
    if ref_con.is_null() || in_number_frames == 0 {
        return NO_ERR;
    }

    let state = unsafe { &*(ref_con.cast::<CallbackState>()) };
    let mut samples = vec![0.0f32; in_number_frames as usize];
    let mut buffer_list = AudioBufferList {
        number_buffers: 1,
        buffers: [AudioBuffer {
            number_channels: 1,
            data_byte_size: (samples.len() * std::mem::size_of::<f32>()) as u32,
            data: samples.as_mut_ptr().cast::<c_void>(),
        }],
    };

    let status = unsafe {
        AudioUnitRender(
            state.audio_unit,
            io_action_flags,
            in_time_stamp,
            INPUT_BUS,
            in_number_frames,
            &mut buffer_list,
        )
    };
    if status != NO_ERR {
        return status;
    }

    let rendered_samples = (buffer_list.buffers[0].data_byte_size as usize
        / std::mem::size_of::<f32>())
    .min(samples.len());
    samples.truncate(rendered_samples);
    let active = match state.active.lock() {
        Ok(active) => active.clone(),
        Err(poison) => {
            eprintln!("[rec] recovered poisoned VoiceProcessingIO active lock in callback");
            poison.into_inner().clone()
        }
    };
    if let Some(active) = active {
        fan_out_mono(samples, &active.frames, &active.chunk_tx, &active.level_tx);
    }
    NO_ERR
}

unsafe fn begin_capture(unit: &PreparedUnit, active: ActiveCapture) -> Result<u128, String> {
    let state = unsafe { &*unit.state_ptr };
    {
        let mut slot = state
            .active
            .lock()
            .map_err(|_| "VoiceProcessingIO active lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("VoiceProcessingIO already recording".into());
        }
        *slot = Some(active);
    }

    let started = std::time::Instant::now();
    let status = unsafe { AudioOutputUnitStart(unit.unit) };
    if status != NO_ERR {
        if let Ok(mut slot) = state.active.lock() {
            *slot = None;
        }
        return Err(status_error("AudioOutputUnitStart", status));
    }

    Ok(started.elapsed().as_millis())
}

unsafe fn stop_capture(unit: &PreparedUnit, reply: Option<mpsc::Sender<(Vec<f32>, u32)>>) {
    let teardown_started = std::time::Instant::now();
    let status = unsafe { AudioOutputUnitStop(unit.unit) };
    if status != NO_ERR {
        eprintln!(
            "[rec] Apple VoiceProcessingIO stop failed: {}",
            status_error("AudioOutputUnitStop", status)
        );
    }

    let state = unsafe { &*unit.state_ptr };
    let active = match state.active.lock() {
        Ok(mut slot) => slot.take(),
        Err(poison) => {
            eprintln!("[rec] recovered poisoned VoiceProcessingIO active lock while stopping");
            poison.into_inner().take()
        }
    };

    let stop_ms = teardown_started.elapsed().as_millis();
    if stop_ms >= 100 {
        eprintln!("[rec] Apple VoiceProcessingIO stop took {stop_ms}ms");
    }

    match (active, reply) {
        (Some(active), Some(reply)) => {
            let data = match active.frames.lock() {
                Ok(frames) => frames.clone(),
                Err(poison) => {
                    eprintln!("[rec] recovered poisoned audio buffer lock while stopping");
                    poison.into_inner().clone()
                }
            };
            let _ = reply.send((data, VOICE_PROCESSING_SAMPLE_RATE));
        }
        (_, Some(reply)) => {
            let _ = reply.send((Vec::new(), VOICE_PROCESSING_SAMPLE_RATE));
        }
        (_, None) => {}
    }
}

unsafe fn teardown_prepared(unit: PreparedUnit) {
    let status = unsafe { AudioUnitUninitialize(unit.unit) };
    if status != NO_ERR {
        eprintln!(
            "[rec] Apple VoiceProcessingIO uninitialize failed: {}",
            status_error("AudioUnitUninitialize", status)
        );
    }
    unsafe {
        dispose_unit(unit.unit, unit.state_ptr);
    }
}

unsafe fn dispose_unit(unit: AudioUnit, state_ptr: *mut CallbackState) {
    if !unit.is_null() {
        let status = unsafe { AudioComponentInstanceDispose(unit) };
        if status != NO_ERR {
            eprintln!(
                "[rec] Apple VoiceProcessingIO dispose failed: {}",
                status_error("AudioComponentInstanceDispose", status)
            );
        }
    }
    if !state_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(state_ptr));
        }
    }
}

fn float_mono_asbd() -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        sample_rate: VOICE_PROCESSING_SAMPLE_RATE as f64,
        format_id: K_AUDIO_FORMAT_LINEAR_PCM,
        format_flags: K_AUDIO_FORMAT_FLAG_IS_FLOAT | K_AUDIO_FORMAT_FLAG_IS_PACKED,
        bytes_per_packet: byte_size_of::<f32>(),
        frames_per_packet: 1,
        bytes_per_frame: byte_size_of::<f32>(),
        channels_per_frame: 1,
        bits_per_channel: 32,
        reserved: 0,
    }
}

fn best_effort_set_u32(
    unit: AudioUnit,
    label: &str,
    property: u32,
    scope: u32,
    element: u32,
    value: u32,
) {
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            property,
            scope,
            element,
            (&value as *const u32).cast::<c_void>(),
            byte_size_of::<u32>(),
        )
    };
    if status != NO_ERR {
        eprintln!("[rec] {label} failed: {}", status_error(label, status));
    }
}

fn best_effort_set_minimum_ducking(unit: AudioUnit) {
    let config = AUVoiceIOOtherAudioDuckingConfiguration {
        enable_advanced_ducking: 0,
        ducking_level: K_AU_VOICE_IO_OTHER_AUDIO_DUCKING_LEVEL_MIN,
    };
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            K_AU_VOICE_IO_PROPERTY_OTHER_AUDIO_DUCKING_CONFIGURATION,
            K_AUDIO_UNIT_SCOPE_GLOBAL,
            0,
            (&config as *const AUVoiceIOOtherAudioDuckingConfiguration).cast::<c_void>(),
            byte_size_of::<AUVoiceIOOtherAudioDuckingConfiguration>(),
        )
    };
    if status != NO_ERR && DUCKING_CONFIG_WARNING.set(()).is_ok() {
        eprintln!(
            "[rec] minimum VoiceProcessingIO ducking config unavailable: {}",
            status_error("OtherAudioDuckingConfiguration", status)
        );
    }
}

fn best_effort_set_stream_format(
    unit: AudioUnit,
    label: &str,
    scope: u32,
    element: u32,
    asbd: &AudioStreamBasicDescription,
) {
    let status = unsafe {
        AudioUnitSetProperty(
            unit,
            K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
            scope,
            element,
            (asbd as *const AudioStreamBasicDescription).cast::<c_void>(),
            byte_size_of::<AudioStreamBasicDescription>(),
        )
    };
    if status != NO_ERR {
        eprintln!("[rec] {label} failed: {}", status_error(label, status));
    }
}

fn check(label: &str, status: OSStatus) -> Result<(), String> {
    if status == NO_ERR {
        Ok(())
    } else {
        Err(status_error(label, status))
    }
}

fn status_error(label: &str, status: OSStatus) -> String {
    format!("{label} status={status}{}", fourcc_status(status))
}

fn fourcc_status(status: OSStatus) -> String {
    if status >= 0 {
        return String::new();
    }

    let raw = status as u32;
    let bytes = raw.to_be_bytes();
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        format!(
            " ('{}')",
            String::from_utf8_lossy(&bytes).trim_end_matches('\0')
        )
    } else {
        String::new()
    }
}

const fn byte_size_of<T>() -> u32 {
    std::mem::size_of::<T>() as u32
}
