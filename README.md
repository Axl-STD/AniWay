# AniWay

Stickers animados para tu escritorio Wayland. Vídeos, GIF, APNG, WebP e imágenes como overlays que dejan pasar los
clics, sobre cualquiera de tus monitores. Colócalos en un mapa de tus pantallas, agrúpalos en escenas con sus propios
atajos y elige para cada uno si queda por encima de las apps a pantalla completa o debajo de tus ventanas.

Un pequeño servicio en segundo plano (`aniwayd`) los dibuja con libmpv y decodificación por hardware; el editor
(`aniway-editor`) y la línea de comandos (`aniway`) solo lo controlan, así que los overlays siguen funcionando con el
editor cerrado.

## Funciones

- Cualquier medio que abra mpv: vídeo (también con transparencia), GIF, APNG, WebP animado e imágenes.
- Editor con mapa de monitores: arrastra, ancla a 9 puntos, escala, opacidad, velocidad y capa
  (sobre todo, sobre las ventanas, bajo las ventanas, fondo de escritorio).
- Sobreposición: deja salir hasta la mitad de un objeto del borde del monitor para ocultar márgenes transparentes.
- Escenas guardadas en TOML, con atajo de teclado propio en Hyprland.
- Copias optimizadas de los medios que se dibujan mucho más pequeños, límite de fotogramas por objeto.
- Icono en la bandeja, pausar u ocultar todo durante un rato, inicio automático opcional.
- Siete temas, interfaz en español e inglés.

## Compatibilidad

| | |
|---|---|
| Hyprland | Probado con 0.57. Todas las funciones. |
| Otros compositores con `wlr-layer-shell` (Sway, river, Wayfire, labwc, niri, KDE Plasma, COSMIC) | Debería funcionar, sin probar. Sin atajos de escena ni reglas por clase de ventana. |
| GNOME, X11 | No compatible (no tienen layer-shell). |

## Compilar

Necesitas Rust (stable), `mpv` (libmpv), GTK 4.14+, libadwaita 1.8+, gettext y `ffmpeg` (`ffprobe` para las
miniaturas y la información de los medios).

```bash
cargo build --release --workspace --bins
```

Los binarios quedan en `target/release/`: `aniwayd`, `aniway` y `aniway-editor`.

## Uso

```bash
target/release/aniway daemon &      # inicia el servicio
target/release/aniway-editor        # abre el editor
target/release/aniway add ~/gif.gif --anchor bottom-right --scale 0.5
target/release/aniway --help        # resto de comandos
```

Para que arranque con la sesión: `aniway autostart on`, o `exec-once` con `aniwayd` en tu configuración de Hyprland.

La configuración y las escenas viven en `~/.config/aniway/`. Más detalles en [`docs/architecture.md`](docs/architecture.md)
y [`docs/editor.md`](docs/editor.md).

## Licencia

[GPL-3.0](LICENSE).
