// Windows-only: read agy CLI's OAuth token from Credential Manager.
// agy stores via zalando/go-keyring as a Generic credential with TargetName="gemini:antigravity".

#[cfg(windows)]
pub fn read_token_blob(target: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let target_w: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut p_cred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        let ok = CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p_cred);
        if ok == 0 || p_cred.is_null() {
            return None;
        }
        let cred = &*p_cred;
        let blob = std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize).to_vec();
        CredFree(p_cred as *mut _);
        Some(blob)
    }
}

#[cfg(not(windows))]
pub fn read_token_blob(_target: &str) -> Option<Vec<u8>> {
    None
}

/// Hash of the credential blob — used by cli_refresher to detect whether agy
/// actually rotated the token, since agy refresh only touches wincred and
/// leaves ~/.gemini/oauth_creds.json untouched.
pub fn read_blob_hash(target: &str) -> Option<u64> {
    let blob = read_token_blob(target)?;
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    blob.hash(&mut hasher);
    Some(hasher.finish())
}
