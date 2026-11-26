(demo "Compute on a worker"
  (setup (window-size 480 272))
  (steps
    (wait 500)
    (click (button "Compute"))
    (click (button "Tap while computing"))
    (await-work)
    (expect-visible (text "Sum of squares modulo 1,000,000,007: 347464"))
    (expect-visible (text "Taps: 1"))
    (wait 1200)))
