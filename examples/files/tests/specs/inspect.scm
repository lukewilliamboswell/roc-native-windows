(test "select, preview and clear a file"
  (steps
    (files "tests/fixtures/hello.txt")
    (await-work)
    (expect-visible (text "hello.txt · 23 bytes"))
    (files)
    (expect-visible (text "hello.txt · 23 bytes"))
    (expect-visible (text "hello from a fixture "))
    (expect-visible (text "SHA-256: 64fa48697ba62a5afa31a7000d648de9a0dd395484cb88bfff1fa3628ab6bc83"))
    (click (button "Clear"))
    (expect-visible (text "Choose a file to preview its contents."))
    (expect-absent (text "hello from a fixture "))
    (expect-absent (text "SHA-256: 64fa48697ba62a5afa31a7000d648de9a0dd395484cb88bfff1fa3628ab6bc83"))))
