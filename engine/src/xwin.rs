//! Ventana X11 anfitriona para GUIs de plugin EMBEBIDAS (CLAP `is_floating = false`).
//!
//! Muchos plugins CLAP de Linux (p. ej. los hechos con nih-plug/baseview) NO
//! soportan ventana flotante: solo exponen la GUI embebida en un XID que debe
//! crear el host. Sin esta ventana, `is_api_supported(X11, floating)` da false
//! y el boton "Interfaz" falla.
//!
//! # Atribucion
//! Patron de creacion de ventana XCB + WM_DELETE_WINDOW adaptado de
//! `vst3-host/src/window.rs` (MIT, Helge Sverre:
//! https://github.com/HelgeSverre/rust-vst3-host). Reimplementado aqui con el
//! crate `xcb` (MIT/Apache-2.0); no se copia codigo de ningun DAW GPL.
//!
//! Todo ocurre en el hilo principal (App::update). Nunca en el callback RT.

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct XWindow {
        conn: xcb::Connection,
        window: xcb::x::Window,
        wm_protocols: xcb::x::Atom,
        wm_delete: xcb::x::Atom,
        close_requested: AtomicBool,
    }

    impl XWindow {
        pub fn new(title: &str, width: u32, height: u32) -> Result<Self, String> {
            use xcb::x;
            let (conn, screen_num) = xcb::Connection::connect(None)
                .map_err(|e| format!("no se pudo conectar al servidor X (¿DISPLAY?): {e}"))?;

            let protocols_cookie = conn.send_request(&x::InternAtom {
                only_if_exists: false, name: b"WM_PROTOCOLS",
            });
            let delete_cookie = conn.send_request(&x::InternAtom {
                only_if_exists: false, name: b"WM_DELETE_WINDOW",
            });
            let wm_protocols = conn.wait_for_reply(protocols_cookie)
                .map_err(|e| format!("WM_PROTOCOLS: {e}"))?.atom();
            let wm_delete = conn.wait_for_reply(delete_cookie)
                .map_err(|e| format!("WM_DELETE_WINDOW: {e}"))?.atom();

            let setup = conn.get_setup();
            let screen = setup.roots().nth(screen_num as usize)
                .ok_or("no hay pantalla X11")?;
            let window: x::Window = conn.generate_id();

            conn.send_and_check_request(&x::CreateWindow {
                depth: x::COPY_FROM_PARENT as u8,
                wid: window,
                parent: screen.root(),
                x: 0,
                y: 0,
                width: width.clamp(1, 16384) as u16,
                height: height.clamp(1, 16384) as u16,
                border_width: 0,
                class: x::WindowClass::InputOutput,
                visual: screen.root_visual(),
                value_list: &[
                    x::Cw::BackPixel(screen.black_pixel()),
                    // SUBSTRUCTURE_NOTIFY: nos enteramos de que el plugin
                    // creo/mapeo su ventana hija.
                    x::Cw::EventMask(x::EventMask::STRUCTURE_NOTIFY | x::EventMask::SUBSTRUCTURE_NOTIFY),
                ],
            }).map_err(|e| format!("crear ventana X11: {e}"))?;

            conn.send_request(&x::ChangeProperty {
                mode: x::PropMode::Replace, window,
                property: x::ATOM_WM_NAME, r#type: x::ATOM_STRING,
                data: title.as_bytes(),
            });
            // Pide al WM un mensaje en vez de matar la ventana: asi podemos
            // soltar la GUI del plugin ANTES de destruir su ventana padre.
            conn.send_and_check_request(&x::ChangeProperty {
                mode: x::PropMode::Replace, window,
                property: wm_protocols, r#type: x::ATOM_ATOM,
                data: &[wm_delete],
            }).map_err(|e| format!("WM_DELETE_WINDOW: {e}"))?;

            conn.send_request(&x::MapWindow { window });
            conn.flush().map_err(|e| format!("flush X11: {e}"))?;

            Ok(Self { conn, window, wm_protocols, wm_delete, close_requested: AtomicBool::new(false) })
        }

        /// XID para `set_parent` (CLAP) / `X11EmbedWindowID`.
        pub fn id(&self) -> u32 { xcb::Xid::resource_id(&self.window) }

        /// Cambia el tamano de la ventana (peticion de resize del plugin).
        pub fn resize(&self, width: u32, height: u32) {
            self.conn.send_request(&xcb::x::ConfigureWindow {
                window: self.window,
                value_list: &[
                    xcb::x::ConfigWindow::Width(width.clamp(1, 16384)),
                    xcb::x::ConfigWindow::Height(height.clamp(1, 16384)),
                ],
            });
            let _ = self.conn.flush();
        }

        /// Drena eventos sin bloquear. true si el usuario pulso el boton de cierre.
        pub fn poll_close(&self) -> bool {
            loop {
                match self.conn.poll_for_event() {
                    Ok(Some(xcb::Event::X(xcb::x::Event::ClientMessage(ev)))) => {
                        if ev.window() == self.window && ev.r#type() == self.wm_protocols {
                            if let xcb::x::ClientMessageData::Data32(d) = ev.data() {
                                if d[0] == xcb::Xid::resource_id(&self.wm_delete) {
                                    self.close_requested.store(true, Ordering::Release);
                                }
                            }
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            self.close_requested.load(Ordering::Acquire)
        }
    }

    impl Drop for XWindow {
        fn drop(&mut self) {
            self.conn.send_request(&xcb::x::UnmapWindow { window: self.window });
            self.conn.send_request(&xcb::x::DestroyWindow { window: self.window });
            let _ = self.conn.flush();
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    /// Sin implementacion fuera de Linux: Windows/macOS usan la GUI flotante.
    pub struct XWindow;
    impl XWindow {
        pub fn new(_t: &str, _w: u32, _h: u32) -> Result<Self, String> {
            Err("ventana embebida solo disponible en Linux/X11".into())
        }
        pub fn id(&self) -> u32 { 0 }
        pub fn resize(&self, _w: u32, _h: u32) {}
        pub fn poll_close(&self) -> bool { false }
    }
}

pub use imp::XWindow;
