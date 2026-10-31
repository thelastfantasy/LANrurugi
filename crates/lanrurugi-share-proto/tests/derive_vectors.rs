//! Fixed vectors for code derivation. They were computed with an independent implementation
//! (Python: argon2-cffi, `hmac`/`hashlib` HKDF, `cryptography` Ed25519), so a change here means
//! codes handed out by an older version would stop resolving.

use lanrurugi_share_proto::code::ShareCode;
use lanrurugi_share_proto::derive::{access_hash, DerivedKeys};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Vector {
    code: &'static str,
    access: &'static str,
    lookup_pub: &'static str,
    record: &'static str,
    lookup_hash: &'static str,
}

const VECTORS: [Vector; 2] = [
    Vector {
        code: "K7QM9-XRT2H",
        access: "4051a4cdd3ccfe39658cdff4c61077044e6404c1a0604bad1a5ba7bea416fb3e",
        lookup_pub: "e591ef75aad802929dd5dd4160871cf8ec785d44b75e11917010367eef0fef0f",
        record: "eef9eb0b9ac71f705b1c88ba5a4066d7c25026b2ff5f363b87fdc6c6dca6b008",
        lookup_hash: "05b094a091fb4cb68a8c8d5b68458be7bdccfaf23a4a851435fcc1ab6022f230",
    },
    Vector {
        code: "k7qm-9xrt",
        access: "48bc341941821d42b4a7c3fcb444f9ea976bbc31e91022e1d69b18730404e420",
        lookup_pub: "af515d8780a7de3824b7d4fe738ea0e88d02bd74bae68fc78dd6aeef259ea5f3",
        record: "0cfb60e4e419b6966789d74fd0725b69a276868b402de7514f1cf81bed62a4f2",
        lookup_hash: "ccc4548c8f97ab2638aacbb1ff40c77e571aba3125397dc166c9a1cc286bace4",
    },
];

#[test]
fn derivation_matches_the_independent_vectors() {
    for v in VECTORS {
        let code = ShareCode::parse(v.code).unwrap();
        assert_eq!(hex(&access_hash(&code)), v.access, "{}", v.code);
        let keys = DerivedKeys::derive(&code);
        assert_eq!(hex(&keys.lookup_public()), v.lookup_pub, "{}", v.code);
        assert_eq!(hex(&keys.record_key), v.record, "{}", v.code);
        assert_eq!(hex(&keys.lookup_hash()), v.lookup_hash, "{}", v.code);
    }
}

#[test]
fn engines_are_compatible_only_when_every_axis_matches() {
    use lanrurugi_share_proto::messages::{ShareEngine, SHARE_ENGINE_VERSION, SHARE_TRANSPORT};

    let current = ShareEngine::current();
    assert_eq!(current.protocol, SHARE_ENGINE_VERSION);
    assert_eq!(current.transport, SHARE_TRANSPORT);
    assert!(current.is_compatible_with(&ShareEngine::current()));
    for other in [
        ShareEngine {
            protocol: SHARE_ENGINE_VERSION - 1,
            transport: SHARE_TRANSPORT.into(),
        },
        ShareEngine {
            protocol: SHARE_ENGINE_VERSION + 1,
            transport: SHARE_TRANSPORT.into(),
        },
        ShareEngine {
            protocol: SHARE_ENGINE_VERSION,
            transport: "iroh 9.9.9".into(),
        },
    ] {
        assert!(!current.is_compatible_with(&other), "{other}");
    }
}
