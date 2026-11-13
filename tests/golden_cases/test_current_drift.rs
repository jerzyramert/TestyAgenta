use super::common::KNOT_TO_MPS;
use sailboat_routing_engine::physics::kinematics::compose_velocity_vectors;

#[test]
fn test_current_drift() {
    let velocity =
        compose_velocity_vectors(5.0 * KNOT_TO_MPS, 0.0, 2.0 * KNOT_TO_MPS, 0.0, 0.0, 0.0)
            .expect("current-drift vector composition succeeds");
    let expected_sog_mps = 5.385_164_807_134_504 * KNOT_TO_MPS;
    let expected_cog_rad = (2.0_f64 / 5.0).atan();

    assert!((velocity.sog_mps - expected_sog_mps).abs() <= 0.001);
    assert!((velocity.cog_rad - expected_cog_rad).abs() <= 0.001);
}
