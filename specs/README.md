# Specs

Cada unidad de trabajo es una spec pequeña, revisable en una sentada.

## Estructura

```
specs/
  0000-roadmap.md            mapa de fases y features (documento vivo)
  NNNN-nombre-corto/
    requirements.md          QUÉ y POR QUÉ: historias + criterios de aceptación (EARS)
    design.md                CÓMO: módulos, tipos, flujos, riesgos
    tasks.md                 pasos implementables, cada uno con su test
```

## Reglas

1. **Numeración por orden de creación**, no por fase. El roadmap dice a qué fase pertenece cada una.
2. **Estados** (en la cabecera de `requirements.md`): `borrador` → `aprobada` → `en curso` → `terminada` | `reemplazada por NNNN`.
3. **Inmutables al terminar.** Si algo cambia después, se abre una spec nueva que referencia a la anterior. Así el histórico cuenta la verdad.
4. **Chicas.** Si `tasks.md` pasa de ~10 tareas, se parte en dos specs.
5. **Todo criterio de aceptación tiene un test** (unitario o de integración) que lo verifica. El `tasks.md` nombra ese test.
6. Una spec se implementa en su propia rama `spec/NNNN-nombre` y entra por PR.
7. **Deuda técnica:** todo atajo o límite conocido que no se resuelve en la spec actual se anota en
   [`TECH-DEBT.md`](TECH-DEBT.md) con un id `TD-NNN`. La spec de cierre `tech-debt-cleanup` paga lo que quede abierto.

## Formato de criterios (EARS)

- `CUANDO <evento>, el sistema DEBE <respuesta>.`
- `MIENTRAS <estado>, el sistema DEBE <respuesta>.`
- `SI <condición no deseada>, ENTONCES el sistema DEBE <respuesta>.`
- `El sistema DEBE <respuesta>.` (siempre)
