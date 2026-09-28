//! API de control local de ProxyRR.
//!
//! HTTP + WebSocket ligados solo a `127.0.0.1`, con token por sesión. Es la única
//! vía por la que UI, CLI y tests hablan con el motor.
//! Esqueleto de la spec 0001; la lógica llega en F1.
