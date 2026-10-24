pub const ARA_DOXYGEN_BUILD: u32 = 0;
pub const ARA_CPU_X86: u32 = 1;
pub const ARA_CPU_ARM: u32 = 0;
pub const kARAInvalidPitchNumber: i32 = -2147483648;
pub const kARAInvalidFrequency: f64 = 0.0;
pub const kARADefaultConcertPitchFrequency: f64 = 440.0;
pub type ARAByte = u8;
pub type ARAInt32 = i32;
pub type ARAInt64 = i64;
pub type ARASize = usize;
pub type ARABool = ARAInt32;
pub type ARAUtf8Char = ::core::ffi::c_char;
pub type ARAUtf8String = *const ARAUtf8Char;
pub type ARATimePosition = f64;
pub type ARATimeDuration = f64;
pub type ARASamplePosition = ARAInt64;
pub type ARASampleCount = ARAInt64;
pub type ARAQuarterPosition = f64;
pub type ARAQuarterDuration = f64;
pub type ARASampleRate = f64;
pub type ARAChannelCount = ARAInt32;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}
pub type ARAPersistentID = *const ::core::ffi::c_char;
pub type ARAAPIGeneration = ARAInt32;
pub const kARAAPIGeneration_1_0_Draft: i32 = 1;
pub const kARAAPIGeneration_1_0_Final: i32 = 2;
pub const kARAAPIGeneration_2_0_Draft: i32 = 3;
pub const kARAAPIGeneration_2_0_Final: i32 = 4;
pub const kARAAPIGeneration_2_X_Draft: i32 = 5;
pub type ARAAssertCategory = ARAInt32;
pub const kARAAssertUnspecified: i32 = 0;
pub const kARAAssertInvalidArgument: i32 = -1;
pub const kARAAssertInvalidState: i32 = -2;
pub const kARAAssertInvalidThread: i32 = -3;
pub type ARAAssertFunction = ::core::option::Option<
    unsafe extern "C" fn(
        category: ARAAssertCategory,
        problematicArgument: *const ::core::ffi::c_void,
        diagnosis: *const ::core::ffi::c_char,
    ),
>;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARADocumentProperties {
    pub structSize: ARASize,
    pub name: ARAUtf8String,
}
pub const kARADocumentPropertiesMinSize: usize = 16;
#[repr(C)]
#[derive(Debug)]
pub struct ARAMusicalContextRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAMusicalContextRef = *mut ARAMusicalContextRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAMusicalContextHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAMusicalContextHostRef = *mut ARAMusicalContextHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAMusicalContextProperties {
    pub structSize: ARASize,
    pub name: ARAUtf8String,
    pub orderIndex: ARAInt32,
    pub color: *const ARAColor,
}
pub const kARAMusicalContextPropertiesMinSize: usize = 8;
#[repr(C)]
#[derive(Debug)]
pub struct ARARegionSequenceRefMarkupType {
    _unused: [u8; 0],
}
pub type ARARegionSequenceRef = *mut ARARegionSequenceRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARARegionSequenceHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARARegionSequenceHostRef = *mut ARARegionSequenceHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARARegionSequenceProperties {
    pub structSize: ARASize,
    pub name: ARAUtf8String,
    pub orderIndex: ARAInt32,
    pub musicalContextRef: ARAMusicalContextRef,
    pub color: *const ARAColor,
}
pub const kARARegionSequencePropertiesMinSize: usize = 28;
#[repr(C)]
#[derive(Debug)]
pub struct ARAAudioSourceRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAAudioSourceRef = *mut ARAAudioSourceRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAAudioSourceHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAAudioSourceHostRef = *mut ARAAudioSourceHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAAudioSourceProperties {
    pub structSize: ARASize,
    pub name: ARAUtf8String,
    pub persistentID: ARAPersistentID,
    pub sampleCount: ARASampleCount,
    pub sampleRate: ARASampleRate,
    pub channelCount: ARAChannelCount,
    pub merits64BitSamples: ARABool,
}
pub const kARAAudioSourcePropertiesMinSize: usize = 48;
#[repr(C)]
#[derive(Debug)]
pub struct ARAAudioModificationRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAAudioModificationRef = *mut ARAAudioModificationRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAAudioModificationHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAAudioModificationHostRef = *mut ARAAudioModificationHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAAudioModificationProperties {
    pub structSize: ARASize,
    pub name: ARAUtf8String,
    pub persistentID: ARAPersistentID,
}
pub const kARAAudioModificationPropertiesMinSize: usize = 24;
#[repr(C)]
#[derive(Debug)]
pub struct ARAPlaybackRegionRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAPlaybackRegionRef = *mut ARAPlaybackRegionRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAPlaybackRegionHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAPlaybackRegionHostRef = *mut ARAPlaybackRegionHostRefMarkupType;
pub type ARAPlaybackTransformationFlags = ARAInt32;
pub const kARAPlaybackTransformationNoChanges: i32 = 0;
pub const kARAPlaybackTransformationTimestretch: i32 = 1;
pub const kARAPlaybackTransformationTimestretchReflectingTempo: i32 = 2;
pub const kARAPlaybackTransformationContentBasedFadeAtTail: i32 = 4;
pub const kARAPlaybackTransformationContentBasedFadeAtHead: i32 = 8;
pub const kARAPlaybackTransformationContentBasedFades: i32 = 12;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAPlaybackRegionProperties {
    pub structSize: ARASize,
    pub transformationFlags: ARAPlaybackTransformationFlags,
    pub startInModificationTime: ARATimePosition,
    pub durationInModificationTime: ARATimeDuration,
    pub startInPlaybackTime: ARATimePosition,
    pub durationInPlaybackTime: ARATimeDuration,
    pub musicalContextRef: ARAMusicalContextRef,
    pub regionSequenceRef: ARARegionSequenceRef,
    pub name: ARAUtf8String,
    pub color: *const ARAColor,
}
pub const kARAPlaybackRegionPropertiesMinSize: usize = 52;
pub type ARAContentUpdateFlags = ARAInt32;
pub const kARAContentUpdateEverythingChanged: i32 = 0;
pub const kARAContentUpdateSignalScopeRemainsUnchanged: i32 = 1;
pub const kARAContentUpdateNoteScopeRemainsUnchanged: i32 = 2;
pub const kARAContentUpdateTimingScopeRemainsUnchanged: i32 = 4;
pub const kARAContentUpdateTuningScopeRemainsUnchanged: i32 = 8;
pub const kARAContentUpdateHarmonicScopeRemainsUnchanged: i32 = 16;
#[repr(C)]
#[derive(Debug)]
pub struct ARAContentReaderRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAContentReaderRef = *mut ARAContentReaderRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAContentReaderHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAContentReaderHostRef = *mut ARAContentReaderHostRefMarkupType;
pub type ARAContentType = ARAInt32;
pub const kARAContentTypeNotes: i32 = 10;
pub const kARAContentTypeTempoEntries: i32 = 20;
pub const kARAContentTypeBarSignatures: i32 = 21;
pub const kARAContentTypeStaticTuning: i32 = 31;
pub const kARAContentTypeKeySignatures: i32 = 42;
pub const kARAContentTypeSheetChords: i32 = 45;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentTimeRange {
    pub start: ARATimePosition,
    pub duration: ARATimeDuration,
}
pub type ARAContentGrade = ARAInt32;
pub const kARAContentGradeInitial: i32 = 0;
pub const kARAContentGradeDetected: i32 = 1;
pub const kARAContentGradeAdjusted: i32 = 2;
pub const kARAContentGradeApproved: i32 = 3;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentTempoEntry {
    pub timePosition: ARATimePosition,
    pub quarterPosition: ARAQuarterPosition,
}
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentBarSignature {
    pub numerator: ARAInt32,
    pub denominator: ARAInt32,
    pub position: ARAQuarterPosition,
}
pub type ARAPitchNumber = ARAInt32;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentNote {
    pub frequency: f32,
    pub pitchNumber: ARAPitchNumber,
    pub volume: f32,
    pub startPosition: ARATimePosition,
    pub attackDuration: ARATimeDuration,
    pub noteDuration: ARATimeDuration,
    pub signalDuration: ARATimeDuration,
}
pub type ARACircleOfFifthsIndex = ARAInt32;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentTuning {
    pub concertPitchFrequency: f32,
    pub root: ARACircleOfFifthsIndex,
    pub tunings: [f32; 12usize],
    pub name: ARAUtf8String,
}
pub type ARAKeySignatureIntervalUsage = ARAByte;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentKeySignature {
    pub root: ARACircleOfFifthsIndex,
    pub intervals: [ARAKeySignatureIntervalUsage; 12usize],
    pub name: ARAUtf8String,
    pub position: ARAQuarterPosition,
}
pub type ARAChordIntervalUsage = ARAByte;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentChord {
    pub root: ARACircleOfFifthsIndex,
    pub bass: ARACircleOfFifthsIndex,
    pub intervals: [ARAChordIntervalUsage; 12usize],
    pub name: ARAUtf8String,
    pub position: ARAQuarterPosition,
}
#[repr(C)]
#[derive(Debug)]
pub struct ARAAudioAccessControllerHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAAudioAccessControllerHostRef = *mut ARAAudioAccessControllerHostRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAAudioReaderHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAAudioReaderHostRef = *mut ARAAudioReaderHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAAudioAccessControllerInterface {
    pub structSize: ARASize,
    pub createAudioReaderForSource: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAAudioAccessControllerHostRef,
            audioSourceHostRef: ARAAudioSourceHostRef,
            use64BitSamples: ARABool,
        ) -> ARAAudioReaderHostRef,
    >,
    pub readAudioSamples: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAAudioAccessControllerHostRef,
            audioReaderHostRef: ARAAudioReaderHostRef,
            samplePosition: ARASamplePosition,
            samplesPerChannel: ARASampleCount,
            buffers: *const *mut ::core::ffi::c_void,
        ) -> ARABool,
    >,
    pub destroyAudioReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAAudioAccessControllerHostRef,
            audioReaderHostRef: ARAAudioReaderHostRef,
        ),
    >,
}
pub const kARAAudioAccessControllerInterfaceMinSize: usize = 32;
#[repr(C)]
#[derive(Debug)]
pub struct ARAArchivingControllerHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAArchivingControllerHostRef = *mut ARAArchivingControllerHostRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAArchiveReaderHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAArchiveReaderHostRef = *mut ARAArchiveReaderHostRefMarkupType;
#[repr(C)]
#[derive(Debug)]
pub struct ARAArchiveWriterHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAArchiveWriterHostRef = *mut ARAArchiveWriterHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAArchivingControllerInterface {
    pub structSize: ARASize,
    pub getArchiveSize: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAArchivingControllerHostRef,
            archiveReaderHostRef: ARAArchiveReaderHostRef,
        ) -> ARASize,
    >,
    pub readBytesFromArchive: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAArchivingControllerHostRef,
            archiveReaderHostRef: ARAArchiveReaderHostRef,
            position: ARASize,
            length: ARASize,
            buffer: *mut ARAByte,
        ) -> ARABool,
    >,
    pub writeBytesToArchive: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAArchivingControllerHostRef,
            archiveWriterHostRef: ARAArchiveWriterHostRef,
            position: ARASize,
            length: ARASize,
            buffer: *const ARAByte,
        ) -> ARABool,
    >,
    pub notifyDocumentArchivingProgress: ::core::option::Option<
        unsafe extern "C" fn(controllerHostRef: ARAArchivingControllerHostRef, value: f32),
    >,
    pub notifyDocumentUnarchivingProgress: ::core::option::Option<
        unsafe extern "C" fn(controllerHostRef: ARAArchivingControllerHostRef, value: f32),
    >,
    pub getDocumentArchiveID: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAArchivingControllerHostRef,
            archiveReaderHostRef: ARAArchiveReaderHostRef,
        ) -> ARAPersistentID,
    >,
}
pub const kARAArchivingControllerInterfaceMinSize: usize = 48;
#[repr(C)]
#[derive(Debug)]
pub struct ARAContentAccessControllerHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAContentAccessControllerHostRef = *mut ARAContentAccessControllerHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAContentAccessControllerInterface {
    pub structSize: ARASize,
    pub isMusicalContextContentAvailable: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            musicalContextHostRef: ARAMusicalContextHostRef,
            contentType: ARAContentType,
        ) -> ARABool,
    >,
    pub getMusicalContextContentGrade: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            musicalContextHostRef: ARAMusicalContextHostRef,
            contentType: ARAContentType,
        ) -> ARAContentGrade,
    >,
    pub createMusicalContextContentReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            musicalContextHostRef: ARAMusicalContextHostRef,
            contentType: ARAContentType,
            range: *const ARAContentTimeRange,
        ) -> ARAContentReaderHostRef,
    >,
    pub isAudioSourceContentAvailable: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            audioSourceHostRef: ARAAudioSourceHostRef,
            contentType: ARAContentType,
        ) -> ARABool,
    >,
    pub getAudioSourceContentGrade: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            audioSourceHostRef: ARAAudioSourceHostRef,
            contentType: ARAContentType,
        ) -> ARAContentGrade,
    >,
    pub createAudioSourceContentReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            audioSourceHostRef: ARAAudioSourceHostRef,
            contentType: ARAContentType,
            range: *const ARAContentTimeRange,
        ) -> ARAContentReaderHostRef,
    >,
    pub getContentReaderEventCount: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            contentReaderHostRef: ARAContentReaderHostRef,
        ) -> ARAInt32,
    >,
    pub getContentReaderDataForEvent: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            contentReaderHostRef: ARAContentReaderHostRef,
            eventIndex: ARAInt32,
        ) -> *const ::core::ffi::c_void,
    >,
    pub destroyContentReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAContentAccessControllerHostRef,
            contentReaderHostRef: ARAContentReaderHostRef,
        ),
    >,
}
pub const kARAContentAccessControllerInterfaceMinSize: usize = 80;
#[repr(C)]
#[derive(Debug)]
pub struct ARAModelUpdateControllerHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAModelUpdateControllerHostRef = *mut ARAModelUpdateControllerHostRefMarkupType;
pub type ARAAnalysisProgressState = ARAInt32;
pub const kARAAnalysisProgressStarted: i32 = 0;
pub const kARAAnalysisProgressUpdated: i32 = 1;
pub const kARAAnalysisProgressCompleted: i32 = 2;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAModelUpdateControllerInterface {
    pub structSize: ARASize,
    pub notifyAudioSourceAnalysisProgress: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAModelUpdateControllerHostRef,
            audioSourceHostRef: ARAAudioSourceHostRef,
            state: ARAAnalysisProgressState,
            value: f32,
        ),
    >,
    pub notifyAudioSourceContentChanged: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAModelUpdateControllerHostRef,
            audioSourceHostRef: ARAAudioSourceHostRef,
            range: *const ARAContentTimeRange,
            flags: ARAContentUpdateFlags,
        ),
    >,
    pub notifyAudioModificationContentChanged: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAModelUpdateControllerHostRef,
            audioModificationHostRef: ARAAudioModificationHostRef,
            range: *const ARAContentTimeRange,
            flags: ARAContentUpdateFlags,
        ),
    >,
    pub notifyPlaybackRegionContentChanged: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAModelUpdateControllerHostRef,
            playbackRegionHostRef: ARAPlaybackRegionHostRef,
            range: *const ARAContentTimeRange,
            flags: ARAContentUpdateFlags,
        ),
    >,
}
pub const kARAModelUpdateControllerInterfaceMinSize: usize = 32;
#[repr(C)]
#[derive(Debug)]
pub struct ARAPlaybackControllerHostRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAPlaybackControllerHostRef = *mut ARAPlaybackControllerHostRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAPlaybackControllerInterface {
    pub structSize: ARASize,
    pub requestStartPlayback: ::core::option::Option<
        unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef),
    >,
    pub requestStopPlayback: ::core::option::Option<
        unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef),
    >,
    pub requestSetPlaybackPosition: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAPlaybackControllerHostRef,
            timePosition: ARATimePosition,
        ),
    >,
    pub requestSetCycleRange: ::core::option::Option<
        unsafe extern "C" fn(
            controllerHostRef: ARAPlaybackControllerHostRef,
            startTime: ARATimePosition,
            duration: ARATimeDuration,
        ),
    >,
    pub requestEnableCycle: ::core::option::Option<
        unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef, enable: ARABool),
    >,
}
pub const kARAPlaybackControllerInterfaceMinSize: usize = 48;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARADocumentControllerHostInstance {
    pub structSize: ARASize,
    pub audioAccessControllerHostRef: ARAAudioAccessControllerHostRef,
    pub audioAccessControllerInterface: *const ARAAudioAccessControllerInterface,
    pub archivingControllerHostRef: ARAArchivingControllerHostRef,
    pub archivingControllerInterface: *const ARAArchivingControllerInterface,
    pub contentAccessControllerHostRef: ARAContentAccessControllerHostRef,
    pub contentAccessControllerInterface: *const ARAContentAccessControllerInterface,
    pub modelUpdateControllerHostRef: ARAModelUpdateControllerHostRef,
    pub modelUpdateControllerInterface: *const ARAModelUpdateControllerInterface,
    pub playbackControllerHostRef: ARAPlaybackControllerHostRef,
    pub playbackControllerInterface: *const ARAPlaybackControllerInterface,
}
pub const kARADocumentControllerHostInstanceMinSize: usize = 88;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARARestoreObjectsFilter {
    pub structSize: ARASize,
    pub documentData: ARABool,
    pub audioSourceIDsCount: ARASize,
    pub audioSourceArchiveIDs: *const ARAPersistentID,
    pub audioSourceCurrentIDs: *const ARAPersistentID,
    pub audioModificationIDsCount: ARASize,
    pub audioModificationArchiveIDs: *const ARAPersistentID,
    pub audioModificationCurrentIDs: *const ARAPersistentID,
}
pub const kARARestoreObjectsFilterMinSize: usize = 60;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAStoreObjectsFilter {
    pub structSize: ARASize,
    pub documentData: ARABool,
    pub audioSourceRefsCount: ARASize,
    pub audioSourceRefs: *const ARAAudioSourceRef,
    pub audioModificationRefsCount: ARASize,
    pub audioModificationRefs: *const ARAAudioModificationRef,
}
pub const kARAStoreObjectsFilterMinSize: usize = 44;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAProcessingAlgorithmProperties {
    pub structSize: ARASize,
    pub persistentID: ARAPersistentID,
    pub name: ARAUtf8String,
}
pub const kARAProcessingAlgorithmPropertiesMinSize: usize = 24;
#[repr(C)]
#[derive(Debug)]
pub struct ARADocumentControllerRefMarkupType {
    _unused: [u8; 0],
}
pub type ARADocumentControllerRef = *mut ARADocumentControllerRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARADocumentControllerInterface {
    pub structSize: ARASize,
    pub destroyDocumentController:
        ::core::option::Option<unsafe extern "C" fn(controllerRef: ARADocumentControllerRef)>,
    pub getFactory: ::core::option::Option<
        unsafe extern "C" fn(controllerRef: ARADocumentControllerRef) -> *const ARAFactory,
    >,
    pub beginEditing:
        ::core::option::Option<unsafe extern "C" fn(controllerRef: ARADocumentControllerRef)>,
    pub endEditing:
        ::core::option::Option<unsafe extern "C" fn(controllerRef: ARADocumentControllerRef)>,
    pub notifyModelUpdates:
        ::core::option::Option<unsafe extern "C" fn(controllerRef: ARADocumentControllerRef)>,
    pub beginRestoringDocumentFromArchive: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            archiveReaderHostRef: ARAArchiveReaderHostRef,
        ) -> ARABool,
    >,
    pub endRestoringDocumentFromArchive: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            archiveReaderHostRef: ARAArchiveReaderHostRef,
        ) -> ARABool,
    >,
    pub storeDocumentToArchive: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            archiveWriterHostRef: ARAArchiveWriterHostRef,
        ) -> ARABool,
    >,
    pub updateDocumentProperties: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            properties: *const ARADocumentProperties,
        ),
    >,
    pub createMusicalContext: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            hostRef: ARAMusicalContextHostRef,
            properties: *const ARAMusicalContextProperties,
        ) -> ARAMusicalContextRef,
    >,
    pub updateMusicalContextProperties: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            musicalContextRef: ARAMusicalContextRef,
            properties: *const ARAMusicalContextProperties,
        ),
    >,
    pub updateMusicalContextContent: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            musicalContextRef: ARAMusicalContextRef,
            range: *const ARAContentTimeRange,
            flags: ARAContentUpdateFlags,
        ),
    >,
    pub destroyMusicalContext: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            musicalContextRef: ARAMusicalContextRef,
        ),
    >,
    pub createAudioSource: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            hostRef: ARAAudioSourceHostRef,
            properties: *const ARAAudioSourceProperties,
        ) -> ARAAudioSourceRef,
    >,
    pub updateAudioSourceProperties: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            properties: *const ARAAudioSourceProperties,
        ),
    >,
    pub updateAudioSourceContent: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            range: *const ARAContentTimeRange,
            flags: ARAContentUpdateFlags,
        ),
    >,
    pub enableAudioSourceSamplesAccess: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            enable: ARABool,
        ),
    >,
    pub deactivateAudioSourceForUndoHistory: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            deactivate: ARABool,
        ),
    >,
    pub destroyAudioSource: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
        ),
    >,
    pub createAudioModification: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            hostRef: ARAAudioModificationHostRef,
            properties: *const ARAAudioModificationProperties,
        ) -> ARAAudioModificationRef,
    >,
    pub cloneAudioModification: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
            hostRef: ARAAudioModificationHostRef,
            properties: *const ARAAudioModificationProperties,
        ) -> ARAAudioModificationRef,
    >,
    pub updateAudioModificationProperties: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
            properties: *const ARAAudioModificationProperties,
        ),
    >,
    pub deactivateAudioModificationForUndoHistory: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
            deactivate: ARABool,
        ),
    >,
    pub destroyAudioModification: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
        ),
    >,
    pub createPlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
            hostRef: ARAPlaybackRegionHostRef,
            properties: *const ARAPlaybackRegionProperties,
        ) -> ARAPlaybackRegionRef,
    >,
    pub updatePlaybackRegionProperties: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            playbackRegionRef: ARAPlaybackRegionRef,
            properties: *const ARAPlaybackRegionProperties,
        ),
    >,
    pub destroyPlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            playbackRegionRef: ARAPlaybackRegionRef,
        ),
    >,
    pub isAudioSourceContentAvailable: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            contentType: ARAContentType,
        ) -> ARABool,
    >,
    pub isAudioSourceContentAnalysisIncomplete: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            contentType: ARAContentType,
        ) -> ARABool,
    >,
    pub requestAudioSourceContentAnalysis: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            contentTypesCount: ARASize,
            contentTypes: *const ARAContentType,
        ),
    >,
    pub getAudioSourceContentGrade: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            contentType: ARAContentType,
        ) -> ARAContentGrade,
    >,
    pub createAudioSourceContentReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            contentType: ARAContentType,
            range: *const ARAContentTimeRange,
        ) -> ARAContentReaderRef,
    >,
    pub isAudioModificationContentAvailable: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
            contentType: ARAContentType,
        ) -> ARABool,
    >,
    pub getAudioModificationContentGrade: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
            contentType: ARAContentType,
        ) -> ARAContentGrade,
    >,
    pub createAudioModificationContentReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioModificationRef: ARAAudioModificationRef,
            contentType: ARAContentType,
            range: *const ARAContentTimeRange,
        ) -> ARAContentReaderRef,
    >,
    pub isPlaybackRegionContentAvailable: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            playbackRegionRef: ARAPlaybackRegionRef,
            contentType: ARAContentType,
        ) -> ARABool,
    >,
    pub getPlaybackRegionContentGrade: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            playbackRegionRef: ARAPlaybackRegionRef,
            contentType: ARAContentType,
        ) -> ARAContentGrade,
    >,
    pub createPlaybackRegionContentReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            playbackRegionRef: ARAPlaybackRegionRef,
            contentType: ARAContentType,
            range: *const ARAContentTimeRange,
        ) -> ARAContentReaderRef,
    >,
    pub getContentReaderEventCount: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            contentReaderRef: ARAContentReaderRef,
        ) -> ARAInt32,
    >,
    pub getContentReaderDataForEvent: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            contentReaderRef: ARAContentReaderRef,
            eventIndex: ARAInt32,
        ) -> *const ::core::ffi::c_void,
    >,
    pub destroyContentReader: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            contentReaderRef: ARAContentReaderRef,
        ),
    >,
    pub createRegionSequence: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            hostRef: ARARegionSequenceHostRef,
            properties: *const ARARegionSequenceProperties,
        ) -> ARARegionSequenceRef,
    >,
    pub updateRegionSequenceProperties: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            regionSequenceRef: ARARegionSequenceRef,
            properties: *const ARARegionSequenceProperties,
        ),
    >,
    pub destroyRegionSequence: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            regionSequenceRef: ARARegionSequenceRef,
        ),
    >,
    pub getPlaybackRegionHeadAndTailTime: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            playbackRegionRef: ARAPlaybackRegionRef,
            headTime: *mut ARATimeDuration,
            tailTime: *mut ARATimeDuration,
        ),
    >,
    pub restoreObjectsFromArchive: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            archiveReaderHostRef: ARAArchiveReaderHostRef,
            filter: *const ARARestoreObjectsFilter,
        ) -> ARABool,
    >,
    pub storeObjectsToArchive: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            archiveWriterHostRef: ARAArchiveWriterHostRef,
            filter: *const ARAStoreObjectsFilter,
        ) -> ARABool,
    >,
    pub getProcessingAlgorithmsCount: ::core::option::Option<
        unsafe extern "C" fn(controllerRef: ARADocumentControllerRef) -> ARAInt32,
    >,
    pub getProcessingAlgorithmProperties: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            algorithmIndex: ARAInt32,
        ) -> *const ARAProcessingAlgorithmProperties,
    >,
    pub getProcessingAlgorithmForAudioSource: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
        ) -> ARAInt32,
    >,
    pub requestProcessingAlgorithmForAudioSource: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            audioSourceRef: ARAAudioSourceRef,
            algorithmIndex: ARAInt32,
        ),
    >,
    pub isLicensedForCapabilities: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            runModalActivationDialogIfNeeded: ARABool,
            contentTypesCount: ARASize,
            contentTypes: *const ARAContentType,
            transformationFlags: ARAPlaybackTransformationFlags,
        ) -> ARABool,
    >,
    pub storeAudioSourceToAudioFileChunk: ::core::option::Option<
        unsafe extern "C" fn(
            controllerRef: ARADocumentControllerRef,
            archiveWriterHostRef: ARAArchiveWriterHostRef,
            audioSourceRef: ARAAudioSourceRef,
            documentArchiveID: *mut ARAPersistentID,
            openAutomatically: *mut ARABool,
        ) -> ARABool,
    >,
}
pub const kARADocumentControllerInterfaceMinSize: usize = 336;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARADocumentControllerInstance {
    pub structSize: ARASize,
    pub documentControllerRef: ARADocumentControllerRef,
    pub documentControllerInterface: *const ARADocumentControllerInterface,
}
pub const kARADocumentControllerInstanceMinSize: usize = 24;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAInterfaceConfiguration {
    pub structSize: ARASize,
    pub desiredApiGeneration: ARAAPIGeneration,
    pub assertFunctionAddress: *mut ARAAssertFunction,
}
pub const kARAInterfaceConfigurationMinSize: usize = 20;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAFactory {
    pub structSize: ARASize,
    pub lowestSupportedApiGeneration: ARAAPIGeneration,
    pub highestSupportedApiGeneration: ARAAPIGeneration,
    pub factoryID: ARAPersistentID,
    pub initializeARAWithConfiguration:
        ::core::option::Option<unsafe extern "C" fn(config: *const ARAInterfaceConfiguration)>,
    pub uninitializeARA: ::core::option::Option<unsafe extern "C" fn()>,
    pub plugInName: ARAUtf8String,
    pub manufacturerName: ARAUtf8String,
    pub informationURL: ARAUtf8String,
    pub version: ARAUtf8String,
    pub createDocumentControllerWithDocument: ::core::option::Option<
        unsafe extern "C" fn(
            hostInstance: *const ARADocumentControllerHostInstance,
            properties: *const ARADocumentProperties,
        ) -> *const ARADocumentControllerInstance,
    >,
    pub documentArchiveID: ARAPersistentID,
    pub compatibleDocumentArchiveIDsCount: ARASize,
    pub compatibleDocumentArchiveIDs: *const ARAPersistentID,
    pub analyzeableContentTypesCount: ARASize,
    pub analyzeableContentTypes: *const ARAContentType,
    pub supportedPlaybackTransformationFlags: ARAPlaybackTransformationFlags,
    pub supportsStoringAudioFileChunks: ARABool,
}
pub const kARAFactoryMinSize: usize = 124;
pub type ARAPlugInInstanceRoleFlags = ARAInt32;
pub const kARAPlaybackRendererRole: i32 = 1;
pub const kARAEditorRendererRole: i32 = 2;
pub const kARAEditorViewRole: i32 = 4;
#[repr(C)]
#[derive(Debug)]
pub struct ARAPlaybackRendererRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAPlaybackRendererRef = *mut ARAPlaybackRendererRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAPlaybackRendererInterface {
    pub structSize: ARASize,
    pub addPlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            playbackRendererRef: ARAPlaybackRendererRef,
            playbackRegionRef: ARAPlaybackRegionRef,
        ),
    >,
    pub removePlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            playbackRendererRef: ARAPlaybackRendererRef,
            playbackRegionRef: ARAPlaybackRegionRef,
        ),
    >,
}
pub const kARAPlaybackRendererInterfaceMinSize: usize = 24;
#[repr(C)]
#[derive(Debug)]
pub struct ARAEditorRendererRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAEditorRendererRef = *mut ARAEditorRendererRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAEditorRendererInterface {
    pub structSize: ARASize,
    pub addPlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            editorRendererRef: ARAEditorRendererRef,
            playbackRegionRef: ARAPlaybackRegionRef,
        ),
    >,
    pub removePlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            editorRendererRef: ARAEditorRendererRef,
            playbackRegionRef: ARAPlaybackRegionRef,
        ),
    >,
    pub addRegionSequence: ::core::option::Option<
        unsafe extern "C" fn(
            editorRendererRef: ARAEditorRendererRef,
            regionSequenceRef: ARARegionSequenceRef,
        ),
    >,
    pub removeRegionSequence: ::core::option::Option<
        unsafe extern "C" fn(
            editorRendererRef: ARAEditorRendererRef,
            regionSequenceRef: ARARegionSequenceRef,
        ),
    >,
}
pub const kARAEditorRendererInterfaceMinSize: usize = 40;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAViewSelection {
    pub structSize: ARASize,
    pub playbackRegionRefsCount: ARASize,
    pub playbackRegionRefs: *const ARAPlaybackRegionRef,
    pub regionSequenceRefsCount: ARASize,
    pub regionSequenceRefs: *const ARARegionSequenceRef,
    pub timeRange: *const ARAContentTimeRange,
}
pub const kARAViewSelectionMinSize: usize = 48;
#[repr(C)]
#[derive(Debug)]
pub struct ARAEditorViewRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAEditorViewRef = *mut ARAEditorViewRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAEditorViewInterface {
    pub structSize: ARASize,
    pub notifySelection: ::core::option::Option<
        unsafe extern "C" fn(editorViewRef: ARAEditorViewRef, selection: *const ARAViewSelection),
    >,
    pub notifyHideRegionSequences: ::core::option::Option<
        unsafe extern "C" fn(
            editorViewRef: ARAEditorViewRef,
            regionSequenceRefsCount: ARASize,
            regionSequenceRefs: *const ARARegionSequenceRef,
        ),
    >,
}
pub const kARAEditorViewInterfaceMinSize: usize = 24;
#[repr(C)]
#[derive(Debug)]
pub struct ARAPlugInExtensionRefMarkupType {
    _unused: [u8; 0],
}
pub type ARAPlugInExtensionRef = *mut ARAPlugInExtensionRefMarkupType;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAPlugInExtensionInterface {
    pub structSize: ARASize,
    pub setPlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            plugInExtensionRef: ARAPlugInExtensionRef,
            playbackRegionRef: ARAPlaybackRegionRef,
        ),
    >,
    pub removePlaybackRegion: ::core::option::Option<
        unsafe extern "C" fn(
            plugInExtensionRef: ARAPlugInExtensionRef,
            playbackRegionRef: ARAPlaybackRegionRef,
        ),
    >,
}
pub const kARAPlugInExtensionInterfaceMinSize: usize = 24;
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct ARAPlugInExtensionInstance {
    pub structSize: ARASize,
    pub plugInExtensionRef: ARAPlugInExtensionRef,
    pub plugInExtensionInterface: *const ARAPlugInExtensionInterface,
    pub playbackRendererRef: ARAPlaybackRendererRef,
    pub playbackRendererInterface: *const ARAPlaybackRendererInterface,
    pub editorRendererRef: ARAEditorRendererRef,
    pub editorRendererInterface: *const ARAEditorRendererInterface,
    pub editorViewRef: ARAEditorViewRef,
    pub editorViewInterface: *const ARAEditorViewInterface,
}
pub const kARAPlugInExtensionInstanceMinSize: usize = 24;
