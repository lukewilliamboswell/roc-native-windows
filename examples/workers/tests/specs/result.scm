(test "settle a real Roc worker"
  (steps
    (click (button "Compute"))
    (await-work)
    (expect-visible (text "Sum of squares modulo 1,000,000,007: 347464"))))
