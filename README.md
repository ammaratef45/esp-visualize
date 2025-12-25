# ESP-VISTUALIZE

## Working with the garage door open

I found myself with an esp32-s3 and a waveshare RGB 64x32 led matrix.

I'm learning rust and I could use a nice visualization of timers, todos, med reminder, internet status, and many more ideas.

Not sure where the project will go but if you are watching, think of it as me studying/working with the garage door open.

Feel free to watch, hangout, or come help/educate!

## Code that I copied but don't fully understand

### The heap_allocator macro

My guess is that we need to do heap allocation because we don't have an OS that can do heap allocation here, I'm not sure though.

Also pondering how much to allocate to the heap? I see from the macro docs that if I pass an attribute `#[ram(reclaimed)]` then the heap will be allocated in a memory region that would otherwise go unused (reclaiming it from the bootloader), why isn't that the default? that sounds like free memory!

## Improvements

- Use BLE (or some other way) to change configurations like wifi SSID and password.
- Add a PCB design to the setup (consider https://www.kicad.org/)