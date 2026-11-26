(demo "Inspect a text file"
  (setup (window-size 480 272))
  (steps
    (wait 600)
    (files "tests/fixtures/hello.txt")
    (await-work)
    (expect-visible (text "hello.txt · 23 bytes"))
    (wait 1400)
    (click (button "Clear"))
    (wait 450)))
