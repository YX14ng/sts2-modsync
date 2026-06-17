// Icono de la app GENERADO por codigo (no hay un binario en el repo): un cuadrado redondeado azul
// —el acento de la UI— con una PILA DE CARTAS blanca (guiño a Slay the Spire, un deckbuilder). Lo usa
// la ventana del GUI (`eframe ... with_icon`, que toma RGBA crudo) y `build.rs` (lo encodea a un `.ico`
// y lo embebe como icono del `.exe` en el Explorador / la barra de tareas).
//
// SOLO usa `std` (y NINGUN `//!` arriba): asi `build.rs` puede `include!`-ar este archivo y reusar la
// MISMA generacion (el icono de la ventana y el del exe quedan identicos) sin depender del crate. Por
// eso el doc de modulo vive en `lib.rs` (`pub mod icon`), no aca.

/// Genera el icono como RGBA (`size*size*4` bytes, fila por fila de arriba a abajo, NO premultiplicado).
pub fn rgba(size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut out = vec![0u8; n * n * 4];
    let s = size as f32;
    let accent = [0x4F_u8, 0x6B, 0xED]; // azul de la UI
    let accent_dark = [0x35_u8, 0x4D, 0xC4]; // un poco mas oscuro (gradiente vertical sutil)
    let bg_r = s * 0.22; // radio de las esquinas del cuadrado de fondo

    // Pila de cartas: rect "portrait", blancas, con opacidad creciente hacia adelante y offset diagonal.
    let card_hw = s * 0.155;
    let card_hh = s * 0.225;
    let card_r = s * 0.045;
    let off = s * 0.105;
    let cards = [
        (s * 0.5 - off, s * 0.5 + off, 0.42_f32), // atras
        (s * 0.5, s * 0.5, 0.70_f32),             // medio
        (s * 0.5 + off, s * 0.5 - off, 1.00_f32), // adelante
    ];
    let aa = (s / 200.0).max(0.8); // ancho del anti-alias, en px

    for y in 0..n {
        for x in 0..n {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            // Fondo: cuadrado redondeado a sangre, con gradiente vertical. El ALFA del icono es la
            // cobertura de ese cuadrado (las cartas van DENTRO, no cambian la silueta).
            let bg_a = rrect_cov([px, py], [s * 0.5, s * 0.5], [s * 0.5, s * 0.5], bg_r, aa);
            let t = py / s;
            let mut rgb = [
                lerp(accent[0] as f32, accent_dark[0] as f32, t),
                lerp(accent[1] as f32, accent_dark[1] as f32, t),
                lerp(accent[2] as f32, accent_dark[2] as f32, t),
            ];
            // Cartas blancas encima (de atras hacia adelante).
            for (cx, cy, op) in cards {
                let ca = rrect_cov([px, py], [cx, cy], [card_hw, card_hh], card_r, aa) * op;
                if ca > 0.0 {
                    for c in rgb.iter_mut() {
                        *c = lerp(*c, 255.0, ca);
                    }
                }
            }
            let i = (y * n + x) * 4;
            out[i] = rgb[0].round() as u8;
            out[i + 1] = rgb[1].round() as u8;
            out[i + 2] = rgb[2].round() as u8;
            out[i + 3] = (bg_a.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Cobertura anti-aliased [0,1] de un punto `p` dentro de un rect REDONDEADO centrado en `c`,
/// media-extension `half`, radio de esquina `r`. SDF de rect redondeado: `<0` adentro.
fn rrect_cov(p: [f32; 2], c: [f32; 2], half: [f32; 2], r: f32, aa: f32) -> f32 {
    let qx = (p[0] - c[0]).abs() - (half[0] - r);
    let qy = (p[1] - c[1]).abs() - (half[1] - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let inside = qx.max(qy).min(0.0);
    let dist = outside + inside - r;
    (0.5 - dist / aa).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_tiene_el_tamano_correcto_y_silueta() {
        let n = 64u32;
        let px = rgba(n);
        assert_eq!(px.len(), (n * n * 4) as usize);
        // El centro es OPACO (dentro del cuadrado) y blanco-ish (carta del frente).
        let c = ((n / 2 * n + n / 2) * 4) as usize;
        assert_eq!(px[c + 3], 255);
        assert!(px[c] > 200 && px[c + 1] > 200 && px[c + 2] > 200);
        // La esquina (0,0) es TRANSPARENTE (afuera del cuadrado redondeado).
        assert_eq!(px[3], 0);
    }
}
