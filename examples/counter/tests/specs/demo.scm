(demo "Change a count"
  (setup (window-size 480 272))
  (steps
    (wait 500)
    (click (button "Increase"))
    (wait 420)
    (click (button "Increase"))
    (expect-visible (text "Count: 2"))
    (wait 650)
    (click (button "Decrease"))
    (wait 450)
    (click (button "Reset"))
    (expect-visible (text "Count: 0"))
    (wait 700)))
