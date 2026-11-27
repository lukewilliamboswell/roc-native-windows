(test "refuse an undeclared origin even in a scenario with virtual HTTP"
  (steps
    (fill (text-field "URL") "https://undeclared.example.test/guide")
    (click (button "Fetch"))
    (expect-visible (text "Network error: HTTP origin is not allowed by this application's capabilities"))
    (expect-pending-effects 0)
    (expect-progress (progress-bar "Request progress") idle)))
