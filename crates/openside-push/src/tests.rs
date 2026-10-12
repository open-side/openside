use super::*;
use crate::fake_push::FakePushService;

fn b64(value: &str) -> Vec<u8> {
    B64.decode(value.replace(' ', "")).unwrap()
}

/// RFC 8291 Appendix A: the published inputs must produce the published body byte for byte.
#[test]
fn encryption_matches_rfc8291_appendix_a() {
    let as_secret =
        SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
    let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
    let ua_public =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    let auth = "BTBZMqHH6r4Tts7J_aSIgg";
    let body = encrypt_with(
        &as_secret,
        salt,
        ua_public,
        auth,
        b"When I grow up, I want to be a watermelon",
    )
    .unwrap();
    let mut expected = b64("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z 9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml mlMoZIIgDll6e3vCYLocInmYWAmS6Tlz AC8wEqKK6PBru3jl7A8");
    assert_eq!(expected.len(), 86);
    expected.extend(b64(
        "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrd irN6GcG7sFz1y1sqLgVi1VhjVkHsUoEs bI_0LpXMuGvnzQ",
    ));
    assert_eq!(body, expected);

    let ua_secret =
        SecretKey::from_slice(&b64("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")).unwrap();
    assert_eq!(
        decrypt(&ua_secret, &b64(auth), &body).unwrap(),
        b"When I grow up, I want to be a watermelon"
    );
}

#[test]
fn tampering_wrong_auth_and_oversized_payloads_fail() {
    let ua = SecretKey::random(&mut OsRng);
    let p256dh = B64.encode(uncompressed(&ua.public_key()));
    let auth = [7_u8; 16];
    let mut body = encrypt(&p256dh, &B64.encode(auth), b"hello").unwrap();
    assert_eq!(decrypt(&ua, &auth, &body).unwrap(), b"hello");
    assert!(decrypt(&ua, &[8_u8; 16], &body).is_err());
    let last = body.len() - 1;
    body[last] ^= 1;
    assert!(decrypt(&ua, &auth, &body).is_err());
    assert!(matches!(
        encrypt(&p256dh, &B64.encode(auth), &vec![0; MAX_PAYLOAD + 1]),
        Err(Error::TooLarge)
    ));
    assert!(validate_keys(&p256dh, &B64.encode(auth)).is_ok());
    assert!(validate_keys(&p256dh, &B64.encode([1_u8; 15])).is_err());
    assert!(validate_keys("AAAA", &B64.encode(auth)).is_err());
}

#[test]
fn vapid_token_verifies_and_rejects_another_key() {
    let key = VapidKey::generate();
    let header = key
        .authorization(
            "https://push.example.net/send/abc?x=1",
            "mailto:ops@example.net",
            1_000,
        )
        .unwrap();
    let (k, claims) = verify_vapid(&header).unwrap();
    assert_eq!(k, key.public_key());
    assert_eq!(claims.aud, "https://push.example.net");
    assert_eq!(claims.exp, 1_000 + 12 * 3600);
    assert_eq!(claims.sub, "mailto:ops@example.net");
    let other = VapidKey::generate().public_key();
    let forged = header.replace(&format!("k={k}"), &format!("k={other}"));
    assert!(verify_vapid(&forged).is_err());
}

#[cfg(unix)]
#[test]
fn vapid_key_is_created_once_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys/push-vapid.key");
    let first = VapidKey::load_or_create(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let second = VapidKey::load_or_create(&path).unwrap();
    assert_eq!(first.public_key(), second.public_key());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let link = path.with_extension("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(
            VapidKey::load_or_create(&link).is_err(),
            "a symlinked key is refused"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            VapidKey::load_or_create(&path).is_err(),
            "a readable-by-others key is refused"
        );
    }
}

#[tokio::test]
async fn fake_service_decrypts_verified_messages_and_reports_gone() {
    let service = FakePushService::spawn().await.unwrap();
    let subscription = service.subscribe();
    let key = VapidKey::generate();
    let sender = Sender::new().unwrap();
    let target = Subscription {
        endpoint: &subscription.endpoint,
        p256dh: &subscription.p256dh,
        auth: &subscription.auth,
    };
    let status = sender
        .send(
            &key,
            &target,
            br#"{"hello":"phone"}"#,
            60,
            "mailto:a@b.c",
            None,
        )
        .await
        .unwrap();
    assert_eq!(status, 201);
    let deliveries = service.wait_for(1, Duration::from_secs(5)).await;
    assert_eq!(deliveries[0].json(), serde_json::json!({"hello":"phone"}));
    assert_eq!(deliveries[0].vapid_key, key.public_key());
    assert_eq!(deliveries[0].ttl, 60);
    service.expire(&subscription.id);
    let status = sender
        .send(
            &key,
            &target,
            b"again",
            60,
            "mailto:a@b.c",
            Some(service.addr()),
        )
        .await
        .unwrap();
    assert_eq!(status, 410);
}
