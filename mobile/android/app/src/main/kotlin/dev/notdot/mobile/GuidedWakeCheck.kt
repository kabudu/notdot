package dev.notdot.mobile

/** Prompts are the source of test labels; they are never sent to the matcher. */
internal class GuidedWakeCheck(phrase: String, similar: List<String>) {
    val prompts: List<String> = buildList {
        repeat(3) { add(phrase) }
        similar.filter { it.isNotBlank() }.distinct().take(3).forEach { name -> repeat(3) { add(name) } }
        add(phrase.removePrefix("Hey ").removePrefix("hey "))
        add("What's the weather today?")
    }
    var index = 0
        private set
    val current get() = prompts.getOrNull(index)
    val complete get() = index == prompts.size
    /** Advance only after this exact prompt's valid result was durably saved. */
    fun saved(step: Int) {
        if (step == index && !complete) index++
    }
}

/** A missing preference selects the candidate in builds made for that experiment. */
internal fun selectedLabDetector(savedId: String?, phoneticBuilt: Boolean): LabDetector =
    LabDetector.entries.firstOrNull { it.id == savedId }
        ?.takeIf { it != LabDetector.PHONETIC || phoneticBuilt }
        ?: if (phoneticBuilt) LabDetector.PHONETIC else LabDetector.MFCC
