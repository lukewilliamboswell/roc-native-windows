(test "keep a comparison window after closing the main window"
  (steps
    (files "tests/fixtures/hello.txt")
    (click (within (window "Files") (button "Open in New Window")))
    (close-window (window "Files"))
    (expect-absent (window "Files"))
    (await-work)
    (expect-text (within (window "Compare hello.txt") (text-area "Contents")) "hello\n  from a fixture\n")
    (select (within (window "Compare hello.txt") (tabs "File details")) "fingerprint")
    (expect-visible (within (window "Compare hello.txt") (text "SHA-256: 64fa48697ba62a5afa31a7000d648de9a0dd395484cb88bfff1fa3628ab6bc83")))
    (close-window (window "Compare hello.txt"))
    (expect-exit)))
