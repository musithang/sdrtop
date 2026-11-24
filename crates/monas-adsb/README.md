# monas-adsb

The 1090 MHz Extended Squitter decoder for the
[Monas workspace](https://github.com/musithang/sdrtop). The current crate
validates DF17 frames and decodes identification, airborne altitude,
encoded CPR coordinates and airborne velocity. It retains the original
bytes for inspection.

Airborne altitude follows the DO-260C Q-bit encoding. It distinguishes
barometric and geometric sources and exposes 100-foot Gillham codes as
raw values. Airborne velocity distinguishes ground components, IAS or
TAS, heading and vertical rate. Speed magnitudes distinguish missing,
zero and saturated reports.

The crate follows the workspace version and has no stability promise of
its own.
