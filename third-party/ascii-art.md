# Start screen aircraft

Ghost's start screen shows one American aircraft, a different one at each launch
(`apps/desktop/src/components/chat/LaunchArt.tsx`). The ten pictures in
`apps/desktop/src/components/chat/launch-art/` are the ten images the owner supplied on
17 November 2026 as `american_aviation_ascii.zip` ("delete all of those and just replace them with
these images"): aircraft drawn in text characters, ten JPEG files of 1168 by 784 pixels. Ghost
did not fetch or generate them, and this file records only what was received: their origin and
terms are the owner's to state.

They replace the fifteen text drawings the start screen opened on from 7 to 17 November 2026,
which were made from public-domain photographs on Wikimedia Commons. None of those drawings or
photographs ships any more; the list of them is in this file's history.

## How an image became a start screen picture

The start screen draws each picture in the theme's own ink on the theme's own pane, so one set of
files serves every theme, dark and light. For that each image is stored as an ink map: grey only,
white where the picture has ink and black where it has none.

- **Drawn on black** (six images): the picture is already light ink on nothing. Its colour
  channels were dropped and its brightness kept exactly as received
  (`jpegtran -grayscale -optimize -progressive -copy none`, which does not decode the image).
- **Drawn on white** (four images): the picture is dark ink on a white page, so it was made grey
  and negated, then saved again at quality 92
  (`magick IN -colorspace Gray -negate -strip -interlace JPEG -quality 92 OUT`).

Nothing was cropped, resized, retouched or redrawn. On a dark theme the six drawn on black look as
received and the four drawn on white show as their negatives; on a light theme it is the other way
round. The conversion ran once, outside the repository.

## Pictures

SHA-256 is of the file as received and of the stored ink map.

| File | Aircraft | Received as | Conversion | SHA-256 (received) | SHA-256 (stored) |
| --- | --- | --- | --- | --- | --- |
| `b-2-spirit.jpg` | B-2 Spirit | `Udmii.jpg` | brightness kept | `177ba1451ccd36eb76ccbfc8f047327f94746f7d6df6254f6403684849e3a6a8` | `7ff03480fa5dd4d9efcb4185b491f448f06d662289510cc74a9398d7cf4dd294` |
| `b-29-superfortress.jpg` | B-29 Superfortress | `mMTpV.jpg` | negated | `043485471e9d4abdf9caee92893f211132c7896740bb0e603c0947c45918ff00` | `bb706c228142fa29f4be34b68694a7a4bc6f711486c5da55429764c8ed9be3f6` |
| `boeing-747-400.jpg` | Boeing 747-400 | `QIkKc.jpg` | brightness kept | `7d6e28d21e7e8987b4a9f444b5755a0622eec1f8b64646833bbf55a80f897676` | `db56744301bfd3db4f698c72b99c95bbfe429f2b0c3d8f5f07c090bfbdca70cd` |
| `boeing-787-dreamliner.jpg` | Boeing 787 Dreamliner | `2oRaL.jpg` | brightness kept | `3f1183bdbad1580209bb92ed2d5a03876653c77208929fcac8987cb863e35e60` | `97aa0fd2361d4ff411c1350daae2760b0d850ceda59d7d3ec81f4ecd9ff93821` |
| `f-22-raptor.jpg` | F-22 Raptor | `98l3o.jpg` | negated | `c341b7f374fa96174f771a4d4bd0506c763969766c940485753d74d6f9ec2cd3` | `ead6951a5d92b58dbcc2e07dbbf33e5a6853eb06583f5a4ed7bb1af1e31b7d52` |
| `f-35-lightning-ii.jpg` | F-35 Lightning II | `HKWaL.jpg` | negated | `3c9892348ceec3d03768e2c4e85c6ba8f33d1860ef71996676534e0a47e38ac1` | `9603ae7c86feeaf08b4b92b159f5b4f0656af98f2f05ba6a8266524cf2a945f2` |
| `p-51-mustang.jpg` | P-51 Mustang | `kvc0V.jpg` | negated | `a7d2dbb59c71277cf6f2e43e448a40465b5a4a65fcbefaed8458a6c668f7f74e` | `c18a77c41bb400d415a55aa592cc5f482a9ab7c08f58e0fb4da0662a29c1353e` |
| `space-shuttle.jpg` | Space Shuttle | `5sEKT.jpg` | brightness kept | `1a6df3b9efa29c63a7b22425637d6d3a57b3478e17a6d4d462531224d6535026` | `306e4e05f8087356e066481bd6674f8167901d0c606f95476e6a4251b9961fe7` |
| `sr-71-blackbird.jpg` | SR-71 Blackbird | `wpWEX.jpg` | brightness kept | `5101df78f52964a4c66468b6698805296836e1e6aa5ae21e16a493ec24d7deaa` | `63f59a4ed1aa13350aedd74876f3c2e3f87c2313819915e5658cf71982970417` |
| `wright-flyer.jpg` | Wright Flyer | `oCuDT.jpg` | brightness kept | `2f2e0fe664e6410199eb52d6dc584d156bdc5e6e2c29844f33528f4127048187` | `663900908a986ce1286628bb831ebc31412d2d46676d0bdc6e0d4d1b51a3a403` |

The Boeing 787 and Boeing 747-400 images carry their own lettering, and the B-29's tail names it;
the other seven are named here by what they show.
