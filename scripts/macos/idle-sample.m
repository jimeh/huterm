// Opt-in external sampler: no messages are sent to Huterm during the interval.
#import <Cocoa/Cocoa.h>
#import <ApplicationServices/ApplicationServices.h>
#include <libproc.h>
#include <mach/mach_time.h>
#include <sys/resource.h>

static BOOL unlocked(void) {
    NSDictionary *session = CFBridgingRelease(CGSessionCopyCurrentDictionary());
    return session && [session[(__bridge NSString *)kCGSessionOnConsoleKey] boolValue]
        && ![session[@"CGSSessionScreenIsLocked"] boolValue];
}

static BOOL counters(pid_t pid, struct rusage_info_v4 *usage, struct proc_taskinfo *task) {
    return proc_pid_rusage(pid, RUSAGE_INFO_V4, (rusage_info_t *)usage) == 0
        && proc_pidinfo(pid, PROC_PIDTASKINFO, 0, task, sizeof(*task)) == sizeof(*task);
}

static void emit(NSDictionary *value) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
    puts([[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding].UTF8String);
}

int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc == 2 && strcmp(argv[1], "context") == 0) {
            NSMutableArray *screens = [NSMutableArray array];
            for (NSScreen *screen in NSScreen.screens) {
                CGDirectDisplayID display = [screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
                CGDisplayModeRef mode = CGDisplayCopyDisplayMode(display);
                [screens addObject:@{@"name":screen.localizedName, @"scale":@(screen.backingScaleFactor),
                    @"frame":NSStringFromRect(screen.frame), @"refresh_hz":@(mode ? CGDisplayModeGetRefreshRate(mode) : 0)}];
                if (mode) CGDisplayModeRelease(mode);
            }
            emit(@{@"unlocked":@(unlocked()), @"screens":screens});
            return 0;
        }
        if (argc == 3 && strcmp(argv[1], "terminate") == 0) {
            return [[NSRunningApplication runningApplicationWithProcessIdentifier:atoi(argv[2])] terminate] ? 0 : 1;
        }
        if (argc != 5) {
            fprintf(stderr, "usage: idle-sample PID visible|hidden SECONDS SETTLE_SECONDS\n");
            return 2;
        }
        pid_t pid = atoi(argv[1]);
        BOOL hidden = strcmp(argv[2], "hidden") == 0;
        double seconds = atof(argv[3]), settle = atof(argv[4]);
        if (pid <= 0 || seconds <= 0 || settle < 0 || (!hidden && strcmp(argv[2], "visible") != 0)) return 2;
        NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
        if (!app || !unlocked()) {
            fprintf(stderr, "target unavailable or GUI session locked/off-console\n");
            return 1;
        }
        if (hidden) [app hide];
        else { [app unhide]; [app activateWithOptions:0]; }
        NSDate *visibilityDeadline = [NSDate dateWithTimeIntervalSinceNow:5];
        while (app.hidden != hidden && visibilityDeadline.timeIntervalSinceNow > 0) {
            [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        }
        if (app.hidden != hidden) {
            fprintf(stderr, "requested visibility was not acknowledged\n");
            return 1;
        }
        // Settling is an explicitly excluded timing interval, not a readiness test.
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:settle]];
        __block BOOL crossedLock = NO;
        id observer = [NSDistributedNotificationCenter.defaultCenter
            addObserverForName:@"com.apple.screenIsLocked" object:nil queue:NSOperationQueue.mainQueue
            usingBlock:^(NSNotification *note) { (void)note; crossedLock = YES; }];
        BOOL beforeUnlocked = unlocked();
        struct rusage_info_v4 before = {0}, after = {0};
        struct proc_taskinfo beforeTask = {0}, afterTask = {0};
        if (!counters(pid, &before, &beforeTask)) return 1;
        uint64_t start = mach_absolute_time();
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:seconds]];
        uint64_t end = mach_absolute_time();
        if (!counters(pid, &after, &afterTask)) return 1;
        BOOL afterUnlocked = unlocked();
        [NSDistributedNotificationCenter.defaultCenter removeObserver:observer];
        mach_timebase_info_data_t timebase;
        mach_timebase_info(&timebase);
        double nanosPerTick = (double)timebase.numer / timebase.denom;
        double elapsed = (end - start) * nanosPerTick / 1e9;
        double cpu = ((after.ri_user_time - before.ri_user_time) + (after.ri_system_time - before.ri_system_time)) * nanosPerTick / 1e9;
        BOOL valid = beforeUnlocked && afterUnlocked && !crossedLock && app.hidden == hidden && !app.terminated;
        emit(@{@"valid":@(valid), @"unlocked_before":@(beforeUnlocked), @"unlocked_after":@(afterUnlocked),
            @"lock_notification":@(crossedLock), @"hidden_after":@(app.hidden), @"elapsed_seconds":@(elapsed),
            @"cpu_seconds":@(cpu), @"cpu_percent_one_core":@(100 * cpu / elapsed),
            @"interrupt_wakeups_per_second":@((after.ri_interrupt_wkups - before.ri_interrupt_wkups) / elapsed),
            @"resident_bytes_before":@(before.ri_resident_size), @"resident_bytes_after":@(after.ri_resident_size),
            @"threads_before":@(beforeTask.pti_threadnum), @"threads_after":@(afterTask.pti_threadnum),
            @"mach_timebase_numer":@(timebase.numer), @"mach_timebase_denom":@(timebase.denom)});
        return valid ? 0 : 1;
    }
}
