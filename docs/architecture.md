# Architecture

Light is currently a small C++23 command-line application. `apps/light` owns the executable entry point,
and `src/app` owns argument handling and user-facing output.

The workspace filesystem and service are not implemented yet. New process or storage boundaries
must be introduced only with the behavior that needs them and with tests for their failure cases.
