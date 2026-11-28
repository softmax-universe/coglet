use super::image_controls::check_resolver;
use gecaos::driver::ResolverConfig;
use std::collections::BTreeMap;
fn cfg()->ResolverConfig {
    ResolverConfig { deterministic:true, tree_strategy_driver:Some("proof".into()),
        files:BTreeMap::from([("strategy".into(),("100644".into(),"a".repeat(64))),
            ("drivers/proof".into(),("100755".into(),"b".repeat(64)))]),..Default::default() }
}
#[test] fn image_resolver_selects_actual_executable() {
    assert_eq!(check_resolver(&cfg()).unwrap(),("drivers/proof".into(),"b".repeat(64)));
}
#[test] fn image_resolver_override_missing_ambiguous_driver_refused() {
    let mut cases=Vec::new();
    let mut c=cfg();c.caos_base=Some("docker:elsewhere".into());cases.push(c);
    let mut c=cfg();c.files.remove("drivers/proof");cases.push(c);
    let mut c=cfg();c.files.insert("drivers/extra".into(),("100755".into(),"c".repeat(64)));cases.push(c);
    let mut c=cfg();c.deterministic=false;cases.push(c);
    let mut c=cfg();c.tree_strategy_driver=Some("missing".into());cases.push(c);
    let mut c=cfg();c.engine_pin=Some("unknown".into());cases.push(c);
    let mut c=cfg();c.files.get_mut("drivers/proof").unwrap().0="100644".into();cases.push(c);
    for c in cases { assert!(check_resolver(&c).is_err()); }
}
