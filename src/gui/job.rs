//! `Job<T>`: un worker de fondo TIPADO (un `mpsc::Receiver<T>` envuelto) con el patron de sondeo
//! uniforme que todas las pestañas repetian a mano (`Option<Receiver<T>>` + un `poll_*` con los
//! mismos brazos `Empty`/`Disconnected`). `spawn` corre un closure que produce UN `T` (workers de
//! un solo resultado); `channel` da el `Sender` para workers que mandan VARIOS mensajes (streaming:
//! progreso de la sync, device-flow de GitHub). `poll` recibe el proximo mensaje y, mientras el job
//! siga vivo, pide `request_repaint` en `Empty` — asi el job AVANZA aunque no haya input (antes
//! algunos `poll_*` se olvidaban el repaint y solo progresaban si otra cosa repintaba la UI).

use eframe::egui;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

pub(super) struct Job<T>(Option<Receiver<T>>);

impl<T> Default for Job<T> {
    fn default() -> Self {
        Job(None)
    }
}

impl<T: Send + 'static> Job<T> {
    /// Lanza un worker de UN resultado: el closure produce un `T`, que se manda y dispara el repaint.
    /// Reemplaza cualquier job anterior en este slot.
    pub(super) fn spawn(&mut self, ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) {
        let tx = self.channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
    }

    /// Prepara el canal y devuelve el `Sender` para un worker que el caller spawnea a mano (y que
    /// puede mandar VARIOS mensajes). El caller hace `request_repaint` tras cada `send`.
    pub(super) fn channel(&mut self) -> Sender<T> {
        let (tx, rx) = channel();
        self.0 = Some(rx);
        tx
    }

    /// Recibe el proximo mensaje si hay. `Empty` => pide repaint (el job sigue) y `None`.
    /// `Disconnected` => limpia el slot y `None`. NO limpia en `Ok`: el caller decide cuando
    /// terminar — un job de 1 resultado llama `clear` tras procesarlo; uno streaming, en su msg final.
    pub(super) fn poll(&mut self, ctx: &egui::Context) -> Option<T> {
        match self.0.as_ref()?.try_recv() {
            Ok(t) => Some(t),
            Err(TryRecvError::Empty) => {
                ctx.request_repaint();
                None
            }
            Err(TryRecvError::Disconnected) => {
                self.0 = None;
                None
            }
        }
    }

    /// Variante de `poll` SIN pedir repaint, para workers STREAMING que manejan su PROPIO heartbeat
    /// throttled (la barra de descarga no quiere repintar a 60fps). `None` en `Empty`; en
    /// `Disconnected` limpia el slot. Tras drenar, mira `busy()` para saber si el worker sigue vivo.
    pub(super) fn next(&mut self) -> Option<T> {
        match self.0.as_ref()?.try_recv() {
            Ok(t) => Some(t),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.0 = None;
                None
            }
        }
    }

    /// Da por terminado el job (limpia el slot).
    pub(super) fn clear(&mut self) {
        self.0 = None;
    }

    /// `true` si hay un job en curso en este slot.
    pub(super) fn busy(&self) -> bool {
        self.0.is_some()
    }
}
