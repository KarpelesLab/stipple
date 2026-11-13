# An NVDA synthesizer for CI: speaks nothing (the runner has no audio), but,
# unlike NVDA's own "silence" synth, reports each index of what it is given as
# reached and then that it is done speaking, as a real synth does. NVDA's
# say all (NVDA+B, read the window) waits for those before reading on, so
# with "silence" it stops after the first object.
#
# .github/scripts/nvda-windows.ps1 copies this into the scratchpad of the
# configuration it starts NVDA with (<config>/scratchpad/synthDrivers/), and
# selects it. NVDA logs everything it says as "Speaking [...]" itself.
# (NVDA's own system tests use the same approach: their speech spy synth.)

import queue
import threading

import synthDriverHandler
from logHandler import log
from speech.commands import IndexCommand


class SynthDriver(synthDriverHandler.SynthDriver):
	name = "stippleSpy"
	description = "Stipple CI (no audio)"
	supportedSettings = ()
	supportedNotifications = {
		synthDriverHandler.synthIndexReached,
		synthDriverHandler.synthDoneSpeaking,
	}

	@classmethod
	def check(cls):
		return True

	def __init__(self):
		super().__init__()
		self._queue = queue.SimpleQueue()
		self._generation = 0
		self._running = True
		self._thread = threading.Thread(target=self._run, name="stippleSpy", daemon=True)
		self._thread.start()
		log.info("stippleSpy: ready")

	def terminate(self):
		self._running = False
		self._queue.put(None)
		self._thread.join(2)

	def speak(self, speechSequence):
		self._queue.put((self._generation, list(speechSequence)))

	def cancel(self):
		# What was queued before is dropped, unspoken, as a real synth drops it.
		self._generation += 1

	def _run(self):
		while self._running:
			item = self._queue.get()
			if item is None:
				return
			generation, sequence = item
			if generation != self._generation:
				continue
			for command in sequence:
				if isinstance(command, IndexCommand):
					synthDriverHandler.synthIndexReached.notify(synth=self, index=command.index)
			if self._queue.empty():
				synthDriverHandler.synthDoneSpeaking.notify(synth=self)
