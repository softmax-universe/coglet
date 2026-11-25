use coglet_proto::signing::{preimage, verify};
use serde_json::Value;

#[test]
fn frozen_signatures_are_verified_with_pinned_authority_and_domain() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../coglet-types-hub0/fixtures/signatures.json"
    ))
    .unwrap();
    for vector in vectors {
        let purpose = vector["statement"]["purpose"].as_str().unwrap();
        let signer = vector["signer"].as_str().unwrap();
        assert_eq!(
            hex::encode(preimage(purpose, &vector["statement"]).unwrap()),
            vector["preimage_hex"]
        );
        let mut signed = vector.clone();
        signed.as_object_mut().unwrap().remove("preimage_hex");
        verify(&signed, purpose, signer).unwrap();
        assert!(verify(&signed, "foreign-purpose", signer).is_err());
        assert!(verify(&signed, purpose, &"00".repeat(32)).is_err());
        signed["statement"]["generation"] = 2.into();
        assert!(verify(&signed, purpose, signer).is_err());
    }
}
