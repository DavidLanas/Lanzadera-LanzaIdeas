// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Estado compartido del kit de Caja: la configuración de los 16 pads
//! que el motor necesita para decidir qué muestra tocar en cada golpe.
//! Vive en el SessionShared y se publica al audio vía el snapshot.

use caja_core::kit::{KitState, PadCfg, PadLive, ParamCfg, NUM_PADS};

/// Crea un KitState por defecto con los 16 pads configurados
/// (equivalente al preset inicial de Caja).
pub fn default_kit_state() -> KitState {
    let mut ks = KitState::default();
    ks.pads.clear();
    ks.live.clear();
    for i in 0..NUM_PADS {
        ks.pads.push(PadCfg {
            cfg: [
                ParamCfg { base: 0, range: 0, prob: 1000 },      // Muestra
                ParamCfg { base: 500, range: 200, prob: 1000 },  // Pitch (centro)
                ParamCfg { base: 1000, range: 0, prob: 1000 },   // Decay (máx = sin recorte)
                ParamCfg { base: 1000, range: 0, prob: 1000 },   // Filtro (abierto)
            ],
            bank: std::array::from_fn(|_| 0),
            pin: None,
            pin_seed: 0,
            gain: 1000,
            pan: 0,
            mute: false,
            solo: false,
            poly: 2,
            choke_group: 0,
            choker: false,
        });
        ks.live.push(PadLive::default_for(i));
    }
    // Choke group para hats: pads 2 y 3 se cortan entre sí
    ks.pads[2].choke_group = 1;
    ks.pads[2].choker = true;
    ks.pads[3].choke_group = 1;
    ks.pads[3].choker = false;
    ks
}
