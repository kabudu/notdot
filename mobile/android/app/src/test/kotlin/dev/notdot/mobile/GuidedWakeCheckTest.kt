package dev.notdot.mobile

import org.junit.Assert.*
import org.junit.Test

class GuidedWakeCheckTest {
    @Test fun selectionRestoresExplicitChoiceAndUsesCandidateForNewExperiment() {
        assertEquals(LabDetector.PHONETIC, selectedLabDetector(null, true))
        assertEquals(LabDetector.PHONETIC, selectedLabDetector("phonetic-v1", true))
        assertEquals(LabDetector.MFCC, selectedLabDetector("mfcc-v1", true))
        assertEquals(LabDetector.MFCC, selectedLabDetector("phonetic-v1", false))
        assertEquals(LabDetector.MFCC, selectedLabDetector(null, false))
    }
    @Test fun promptsAutomaticallyLabelEveryStep() {
        val run = GuidedWakeCheck("Hey Ava", listOf("Hey Ada", "Hey Eva", "Hey Nova"))
        assertEquals(14, run.prompts.size)
        val expected = List(3) { "Hey Ava" } + List(3) { "Hey Ada" } + List(3) { "Hey Eva" } + List(3) { "Hey Nova" } + listOf("Ava", "What's the weather today?")
        expected.forEachIndexed { index, phrase ->
            assertEquals(phrase, run.current)
            run.saved(index)
        }
        assertTrue(run.complete)
        assertNull(run.current)
    }
    @Test fun lateOrDuplicateSaveCannotSkipAPrompt() {
        val run = GuidedWakeCheck("Hey Ava", emptyList())
        run.saved(3)
        assertEquals(0, run.index)
        run.saved(0)
        run.saved(0)
        assertEquals(1, run.index)
    }
}
