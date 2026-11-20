//! `main/security/*`: подтверждение сборки для онлайна — отпечаток исполняемого файла, ключ
//! подписи под маской и рукопожатие с сервером ради билета; официальная ли сборка для окна.

pub mod attest_seal;
pub mod attestation;
pub mod build_identity;
pub mod build_key;
pub mod official;
