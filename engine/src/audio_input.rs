//! Entrada de audio con cpal (Fase F2).
//! Abre un InputStream de-input, deposita muestras en un ring buffer lockfree
//! que el hilo RT drena en render(). Sin alloc en el callback: el Producer
//! se mueve al closure, el Consumer se guarda en el Renderer.
use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::{HeapCons, HeapRb, traits::{Split, Producer}};
use cpal::traits::HostTrait;

/// Stream de entrada que se mantiene vivo.
pub struct AudioInputStream {
    #[allow(dead_code)]
    stream: cpal::Stream,
    pub channels: u16,
    pub sample_rate: u32,
    pub device_name: String,
}

/// Lista los nombres de dispositivos de entrada disponibles.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for d in devices {
            if let Ok(n) = d.name() {
                names.push(n);
            }
        }
    }
    names
}

/// Abre el stream de entrada por defecto (o por nombre si se indica).
/// Devuelve (stream, consumer).
pub fn open(
    host: &cpal::Host,
    ring_capacity: usize,
) -> Result<(AudioInputStream, HeapCons<f32>), String> {
    open_named(host, ring_capacity, None)
}

/// Abre entrada por nombre de dispositivo. `None` = default del sistema.
pub fn open_named(
    host: &cpal::Host,
    ring_capacity: usize,
    device_name: Option<&str>,
) -> Result<(AudioInputStream, HeapCons<f32>), String> {
    let device = match device_name {
        Some(name) => {
            host.input_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.name().ok().as_deref() == Some(name))
                .ok_or_else(|| format!("dispositivo de entrada no encontrado: {name}"))?
        }
        None => host.default_input_device().ok_or("sin dispositivo de entrada")?,
    };
    let resolved_name = device.name().unwrap_or_else(|_| "default".into());
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = config.channels();
    let sample_rate = config.sample_rate().0;
    let stream_config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let ring = HeapRb::new(ring_capacity);
    let (prod, cons) = ring.split();

    let mut prod = prod;
    let stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = prod.push_slice(data);
            },
            |err| eprintln!("[audio_input] error: {}", err),
            None,
        )
        .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;

    Ok((
        AudioInputStream {
            stream,
            channels,
            sample_rate,
            device_name: resolved_name,
        },
        cons,
    ))
}
