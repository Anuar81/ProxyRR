//! Datos de red para las guías y un QR en texto para la terminal.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

/// IPv4 de esta máquina en la LAN (la de la ruta por defecto), o `None` si no hay red.
/// `connect` en UDP no manda paquetes: solo le pregunta al SO qué interfaz usaría.
#[must_use]
pub fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// QR de `text` con bloques Unicode, para imprimir en la terminal (claro sobre oscuro invertido,
/// que es lo que leen bien las cámaras en terminales de fondo oscuro y claro).
#[must_use]
pub fn qr_text(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

/// QR de `text` como SVG autocontenido (para la app de escritorio).
#[must_use]
pub fn qr_svg(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_svg_is_an_svg() {
        let svg = qr_svg("http://192.168.0.2:9090/cert").unwrap();
        assert!(svg.contains("<svg") && svg.contains("</svg>"));
        assert!(!svg.contains("<script"), "sin scripts");
    }

    #[test]
    fn lan_ip_is_never_loopback() {
        if let Some(ip) = lan_ip() {
            assert!(!ip.is_loopback() && !ip.is_unspecified(), "{ip}");
        }
    }

    #[test]
    fn qr_renders_blocks() {
        let qr = qr_text("http://proxyrr.cert/").unwrap();
        assert!(qr.lines().count() > 10);
        assert!(qr.chars().any(|c| matches!(c, '█' | '▀' | '▄')));
    }
}
