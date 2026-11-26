(test "cancel a held HTTP request"
  (steps
    (click (button "Fetch"))
    (expect-request 1 :method "GET")
    (click (button "Cancel"))
    (expect-cancelled 1)
    (expect-pending-effects 0)
    (expect-visible (text "Cancelled."))))
