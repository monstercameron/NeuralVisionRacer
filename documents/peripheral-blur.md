# Peripheral motion blur refinement

The original blur scaled against the camera reference speed of 250 km/h and used short 0.06 UV streaks. At normal street speeds, the triangular sampling weights reduced the visible smear to only a few pixels.

Peripheral blur now has its own response: it starts above 8 km/h, rises with a 0.7 power curve, and reaches full strength at 110 km/h. Default strength is 0.85 with streak length 0.14; camera shake, FOV and chromatic aberration keep their existing reference speed. At 30/60/100 km/h the resulting shader strength is 0.290/0.530/0.791. Rest and disabled blur produce zero strength. Reverse uses absolute vehicle speed.

The WGSL post shader takes 12 radial samples at the periphery. Visible car depth plus a soft rounded region around the projected car protects the body, including holes in the Gaussian core depth mask. Sampling rejects visible car pixels so paint does not smear into the road. The road centre and HUD remain clear. This is a speed-driven radial effect rather than a temporal motion-vector blur.

Pause menu: Speed blur controls effects.blur. Live console controls also include effects.blur_start_kmh, effects.blur_full_kmh, effects.blur_length and effects.blur_start. The effects master still bypasses everything. Profiler output includes the current blur strength, streak length and radial start in the shared post pass.

Validation: 41 renderer tests pass, plus the engine speed-curve test. GPU checks cover visible peripheral smearing, car/occluder separation, projected protection over depth holes and clean bypass. Matched production-renderer scene frames at 0, 30, 60 and 100 km/h were visually inspected; the scene, car pose and camera are frozen to isolate the effect, with DOF/grain off. Release WASM rebuilt and checksum-verified against localhost:8800.
