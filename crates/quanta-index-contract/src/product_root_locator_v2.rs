//! The durable locator shared by Semantica product owners and offline recovery.
//! The directory identity must come from the caller's own opened root.

pub const PRODUCT_ROOT_IDENTITY_FILE_V2: &str = ".semantica-search-product-root-identity-v2";
const IDENTITY_DOMAIN_V2: &[u8] = b"semantica-search-product-root-identity-v2\0";
const LOCATOR_DOMAIN_V2: &[u8] = b"semantica-search-product-root-locator-v2\0";
pub const PRODUCT_ROOT_IDENTITY_BYTES_V2: usize = IDENTITY_DOMAIN_V2.len() + 1 + 32;

#[derive(Clone, Copy)]
pub enum ProductRootKindV2 {
    SearchPlane = 1,
    RepoMap = 2,
}

#[must_use]
pub fn encode_product_root_identity_v2(
    kind: ProductRootKindV2,
    nonce: [u8; 32],
) -> [u8; PRODUCT_ROOT_IDENTITY_BYTES_V2] {
    let mut bytes = [0; PRODUCT_ROOT_IDENTITY_BYTES_V2];
    bytes[..IDENTITY_DOMAIN_V2.len()].copy_from_slice(IDENTITY_DOMAIN_V2);
    bytes[IDENTITY_DOMAIN_V2.len()] = kind as u8;
    bytes[IDENTITY_DOMAIN_V2.len() + 1..].copy_from_slice(&nonce);
    bytes
}

#[must_use]
pub fn is_product_root_identity_v2(marker: &[u8], kind: ProductRootKindV2) -> bool {
    marker.len() == PRODUCT_ROOT_IDENTITY_BYTES_V2
        && marker.starts_with(IDENTITY_DOMAIN_V2)
        && marker[IDENTITY_DOMAIN_V2.len()] == kind as u8
}

#[must_use]
pub fn decode_product_root_locator_v2(
    marker: &[u8],
    kind: ProductRootKindV2,
    device: u64,
    inode: u64,
) -> Option<[u8; 64]> {
    if !is_product_root_identity_v2(marker, kind) {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    let _state = hasher
        .update(LOCATOR_DOMAIN_V2)
        .update(&(PRODUCT_ROOT_IDENTITY_BYTES_V2 as u64).to_le_bytes())
        .update(marker)
        .update(&device.to_le_bytes())
        .update(&inode.to_le_bytes());
    let mut digest = [0; 64];
    digest.copy_from_slice(hasher.finalize().to_hex().as_bytes());
    Some(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repomap_locator_matches_existing_semantica_marker() {
        let nonce = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31,
        ];
        let mut marker = *b"semantica-search-product-root-identity-v2\0\x02\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f";
        let device = 0x1020_3040_5060_7080;
        let inode = 0xa1b2_c3d4_e5f6_0718;
        let golden = *b"8e8aa5a8c46e7b16fad95579e5e2db1847d2e92c3d9dee3bb4422c9d120233be";
        assert_eq!(
            encode_product_root_identity_v2(ProductRootKindV2::RepoMap, nonce),
            marker
        );
        assert_eq!(
            decode_product_root_locator_v2(&marker, ProductRootKindV2::RepoMap, device, inode),
            Some(golden)
        );
        assert_ne!(
            decode_product_root_locator_v2(&marker, ProductRootKindV2::RepoMap, device, inode + 1),
            Some(golden)
        );
        assert!(
            decode_product_root_locator_v2(&marker, ProductRootKindV2::SearchPlane, device, inode)
                .is_none()
        );
        marker[0] ^= 1;
        assert!(
            decode_product_root_locator_v2(&marker, ProductRootKindV2::RepoMap, device, inode)
                .is_none()
        );
    }
}
