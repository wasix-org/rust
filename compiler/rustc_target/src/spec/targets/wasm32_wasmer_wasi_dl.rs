//! `wasm32-wasmer-wasi` with support for dynamic linking.

use crate::spec::{Env, RelocModel, Target, TlsModel};

pub(crate) fn target() -> Target {
    let mut target = super::wasm32_wasmer_wasi::target();

    target.env = Env::Other("dl".into());

    target.options.dll_prefix = "lib".into();
    target.options.dll_suffix = ".so".into();

    target.options.relocation_model = RelocModel::Pic;
    target.options.tls_model = TlsModel::GeneralDynamic;
    target.options.crt_static_default = false;
    target.options.position_independent_executables = true;

    target
}
