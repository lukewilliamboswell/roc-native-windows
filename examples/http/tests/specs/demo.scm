(demo "Fetch a local response"
  (setup (window-size 480 272))
  (steps
    (wait 450)
    (fill (text-field "URL") "http://127.0.0.1:48393/guide.txt")
    (wait 400)
    (click (button "Fetch"))
    (await-work)
    (expect-visible (text "The local guide is ready."))
    (wait 1400)))
