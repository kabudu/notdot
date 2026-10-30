Optional Windows wake acceleration

build_windows_wake_helper.py stages the generated helper here before bundling.
The normal app stays generic. A missing helper, unsupported CPU or invalid
compiled-in digest keeps the generic on-device recognizer.

The helper never receives voice credentials and does not use network services.
