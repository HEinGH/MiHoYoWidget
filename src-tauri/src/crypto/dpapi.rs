//! Windows DPAPI 加密/解密（与老版 C# CookieManager 完全兼容）
use windows::core::PWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};

/// CRYPTPROTECT_UI_FORBIDDEN = 1（静默，不弹 UI）
const UI_FORBIDDEN: u32 = 1;

/// CryptProtectData 加密（当前用户域）
pub fn encrypt(data: &[u8]) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &in_blob,
            PWSTR::null(),
            None,
            None,
            None,
            UI_FORBIDDEN,
            &mut out_blob,
        )?;
        let result =
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out_blob.pbData as *mut core::ffi::c_void)));
        Ok(result)
    }
}

/// CryptUnprotectData 解密
pub fn decrypt(blob: &[u8]) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            UI_FORBIDDEN,
            &mut out_blob,
        )?;
        let result =
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out_blob.pbData as *mut core::ffi::c_void)));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let plain = b"hello dpapi \xe6\xb5\x8b\xe8\xaf\x95";
        let enc = encrypt(plain).expect("encrypt");
        let dec = decrypt(&enc).expect("decrypt");
        assert_eq!(plain.as_slice(), dec.as_slice());
    }
}
