(demo "Add and complete to-dos"
  (setup (window-size 480 272))
  (steps
    (wait 500)
    (fill (text-field "New to-do") "Review the proposal")
    (click (button "Add"))
    (wait 450)
    (fill (text-field "New to-do") "Send the agenda")
    (click (button "Add"))
    (wait 650)
    (toggle (checkbox "Review the proposal"))
    (expect-visible (text "1 remaining"))
    (wait 1000)))
