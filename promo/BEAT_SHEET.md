# Anuncio Chispa — beat sheet y mapa semántico

**Entregable:** `chispa-anuncio.mp4`, 1920x1080, 16:9, 30 fps, ~32 s, H.264 + AAC.
- Uso: README/GitHub, YouTube y redes horizontales.
- Sin voz en off: el texto en pantalla es el mensaje y el sonido es diseño sintetizado.
- Fuente de hechos: `README.md` y el benchmark del 26/11/2026. No hay footage ni capturas.
- Color: es una pieza 100 % gráfica, sin fuente de video que preservar. La paleta es la de la marca Chispa: fondo `#111113` y acento `#ff7a1a`.

## Beats

| # | Tiempo | Proposición | Mecanismo | Texto permitido | Fuente |
|---|---|---|---|---|---|
| 1 | 0,0–3,2 | Las PCs flojas se traban al transmitir | Hook con *stutter* (demostración abstracta, sin cifras) | «¿Tu PC se traba cuando transmitís?» | Brief del producto |
| 2 | 3,2–6,6 | Chispa es la respuesta | Revelado de la entidad: el rayo se arma y aparece la palabra | «Chispa» / «Grabá, clipeá y transmití en PCs flojas.» | README |
| 3 | 6,6–11,6 | Detecta la PC y elige el perfil | Mapa de estados: 3 detecciones → perfil | «Se configura sola» · CPU 4 hilos · RAM 8 GB · GPU RX 560 → Perfil Potato · 720p 30 fps · encoder por GPU · «PC de prueba real» | README (perfiles) y log del motor |
| 4 | 11,6–16,6 | Se maneja con la voz o con atajos | Transferencia: frase → acción → confirmación | «Manejala con la voz» · «Chispa, clipeá eso» → «Clip guardado · últimos 30 s» · «o con Alt+F10» | README (frases y atajos), mensaje real del motor |
| 5 | 16,6–21,6 | Multistream con un solo encode | Transferencia de conexión 1 → 3 | «Tres plataformas. La CPU de una.» · «1 encode» → Twitch · Kick · YouTube | README / `core/src/live.rs` |
| 6 | 21,6–27,0 | Usa menos RAM que OBS | Sistema de comparación | OBS 32.0.4 166 MB · Chispa 92 MB · «−45 % de RAM» · nota: misma escena, RX 560 · 4 hilos · 8 GB | Benchmark del README |
| 7 | 27,0–32,0 | CTA | Cierre de la entidad | «Gratis y open source» · «github.com/juanipro1979/chispa» · «Windows 10/11» | Repo público, licencia GPL |

## Mapa semántico: estados y restricciones

- Las plataformas se muestran como **chips de texto con color propio**, sin logos oficiales. El estado «recibe» es interno de la animación: no se afirma que se haya transmitido en vivo, porque eso sigue sin probar.
- La cifra de −45 % sale de (166 − 92) / 166 = 44,6 %, redondeado como en el README. La nota al pie aclara que es la misma escena en la PC de prueba.
- La frase de voz y la respuesta son textos reales de la app (`voice.rs`, `bus.rs`).
- «PC de prueba real» identifica que CPU/RAM/GPU son de una máquina concreta y no una promesa general.
- No se usa ninguna afirmación que no esté en el README, como «más rápido que Streamlabs» o la cantidad de usuarios.

## Sonido: eventos

| Evento | Frame (s) | Movimiento | Efecto | Motivo |
|---|---|---|---|---|
| Stutter | 0,4–2,6 | La línea salta | Blips cortos y secos | Hace audible el lag |
| Revelado del rayo | 3,4 | Escala + brillo | Campanita «chispa» (senos con armónicos) | Identidad |
| Transiciones | 6,6 · 11,6 · 16,6 · 21,6 · 27,0 | Salida hacia arriba | Whoosh corto (ruido filtrado en barrido) | Desplazamiento real |
| Detecciones | 7,3 · 7,7 · 8,1 | Aparece cada chip | Tick | Confirmación |
| Perfil listo | 9,4 | Llega la tarjeta resultado | Acorde corto | Resultado |
| Tipeo de la frase | 12,3–13,8 | Se escriben las letras | Ticks suaves | Operación |
| Clip guardado | 14,4 | Aparece la confirmación | Clic de obturador | Confirmación |
| Líneas a las plataformas | 18,05 · 18,35 · 18,65 | Llega cada conector | Ping ascendente | Recepción |
| Barras | 22,4–24,0 | Crecen | Silencio + un acento al −45 % | Payoff |
| CTA | 27,6 | Logo | Campanita | Cierre |

Hay un pad grave de fondo a −26 dBFS RMS que entra y sale con fade. El objetivo de master es un pico ≤ −1 dBFS.
