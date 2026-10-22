# PidgeIoT

An edge-native IoT platform: devices report through it, and people manage their fleets on it.

## Language

### Schema

**Migration**:
The operator-applied file under `infra/migrations/`, and the intended way a schema change reaches
staging and production.
_Avoid_: Bootstrap

**Ensure**:
The per-isolate check that the schema the deployed code needs is present, creating only what is
missing.
_Avoid_: Bootstrap, lazily applied, lazy DDL, belt-and-suspenders
