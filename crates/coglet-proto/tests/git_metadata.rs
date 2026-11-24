use coglet_proto::git_metadata::{MetadataError, Observation, RefKind, VerifiedRef};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};

fn signed(key: &SigningKey, kind: RefKind, hub: &str, coglet: &str) -> Value {
    let statement = json!({"purpose":kind.purpose(), "hub":hub,"coglet":coglet,
        "ref":kind.reference(),"old":null,"new":"ab".repeat(20),
        "archive_sha256":"cd".repeat(32),"generation":1});
    let signature = key.sign(&coglet_proto::signing::preimage(kind.purpose(), &statement).unwrap());
    json!({"statement":statement,"signer":hex::encode(key.verifying_key().as_bytes()),
        "signature":hex::encode(signature.to_bytes())})
}

fn resign(key: &SigningKey, mut value: Value) -> Value {
    value["signature"] = json!(hex::encode(
        key.sign(
            &coglet_proto::signing::preimage(
                value["statement"]["purpose"].as_str().unwrap(),
                &value["statement"]
            )
            .unwrap()
        )
        .to_bytes()
    ));
    value
}

#[test]
fn observation_skips_generations_but_cas_does_not_skip_old_tip() {
    let key = SigningKey::from_bytes(&[3; 32]);
    let hub = "11".repeat(32);
    let coglet = format!("coglet:{}", hex::encode(key.verifying_key().as_bytes()));
    let a = signed(&key, RefKind::Published, &hub, &coglet);
    let verify = |v: &Value| VerifiedRef::verify(v, RefKind::Published, &hub, &coglet).unwrap();
    let first = verify(&a);
    assert_eq!(first.check_observation(None).unwrap(), Observation::Advance);
    assert_eq!(
        first.check_observation(Some(&first)).unwrap(),
        Observation::Identical
    );
    assert!(first.check_authoritative(None, None).is_ok());
    assert_eq!(
        first.check_authoritative(Some(first.new_tip()), Some(1)),
        Err(MetadataError::Conflict)
    );
    let mut c = a.clone();
    c["statement"]["old"] = json!("ef".repeat(20));
    c["statement"]["new"] = json!("12".repeat(20));
    c["statement"]["generation"] = json!(u64::MAX);
    let latest = verify(&resign(&key, c));
    assert_eq!(
        latest.check_observation(Some(&first)).unwrap(),
        Observation::Advance
    );
    assert_eq!(
        first.check_observation(Some(&latest)),
        Err(MetadataError::Conflict)
    );
    assert_eq!(
        latest.check_authoritative(Some(first.new_tip()), Some(1)),
        Err(MetadataError::Conflict)
    );
    assert_eq!(
        latest.check_authoritative(latest.old_tip(), None),
        Err(MetadataError::Conflict)
    );
    assert!(
        latest
            .check_authoritative(latest.old_tip(), Some(1))
            .is_ok()
    );
    let mut changed = a.clone();
    changed["statement"]["old"] = json!("ff".repeat(20));
    let changed = verify(&resign(&key, changed));
    assert_eq!(
        changed.check_observation(Some(&first)),
        Err(MetadataError::Conflict)
    );
    let other = signed(&key, RefKind::Logs, &hub, &coglet);
    assert_eq!(
        verify_ref(&other, RefKind::Logs, &hub, &coglet).check_observation(Some(&first)),
        Err(MetadataError::Conflict)
    );
}
fn verify_ref(v: &Value, kind: RefKind, hub: &str, coglet: &str) -> VerifiedRef {
    VerifiedRef::verify(v, kind, hub, coglet).unwrap()
}

#[test]
fn signed_metadata_pins_hub_target_ref_purpose_and_authoritative_node_key() {
    let hub_key = SigningKey::from_bytes(&[1; 32]);
    let coglet_key = SigningKey::from_bytes(&[2; 32]);
    let hub = hex::encode(hub_key.verifying_key().as_bytes());
    let coglet = format!(
        "coglet:{}",
        hex::encode(coglet_key.verifying_key().as_bytes())
    );
    for kind in [RefKind::Hub, RefKind::Published, RefKind::Logs] {
        let key = if kind == RefKind::Hub {
            &hub_key
        } else {
            &coglet_key
        };
        let value = signed(key, kind, &hub, &coglet);
        let verified = VerifiedRef::verify(&value, kind, &hub, &coglet).unwrap();
        assert_eq!(verified.generation(), 1);
        assert_eq!(verified.new_tip(), "ab".repeat(20));
        assert!(VerifiedRef::verify(&value, kind, &"ff".repeat(32), &coglet).is_err());
        assert!(
            VerifiedRef::verify(&value, kind, &hub, &format!("coglet:{}", "ff".repeat(32)))
                .is_err()
        );
        let wrong = if kind == RefKind::Hub {
            RefKind::Published
        } else {
            RefKind::Hub
        };
        assert!(VerifiedRef::verify(&value, wrong, &hub, &coglet).is_err());
        let untrusted = signed(
            if kind == RefKind::Hub {
                &coglet_key
            } else {
                &hub_key
            },
            kind,
            &hub,
            &coglet,
        );
        assert!(VerifiedRef::verify(&untrusted, kind, &hub, &coglet).is_err());
        let mut tampered = value.clone();
        tampered["statement"]["new"] = json!("ef".repeat(20));
        assert!(VerifiedRef::verify(&tampered, kind, &hub, &coglet).is_err());
    }
    assert!(VerifiedRef::verify(&json!({}), RefKind::Published, &hub, "short").is_err());
}
