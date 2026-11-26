// A small iPhone app for ghost-devices' simulator test (tests/ios.rs): a large button that says
// hello, a label that says what happened, and a field that says what was typed when Return is
// pressed. Built without Xcode's project files by build.sh.
#import <UIKit/UIKit.h>

@interface ViewController : UIViewController <UITextFieldDelegate>
@property(nonatomic, strong) UILabel *label;
@property(nonatomic, strong) UITextField *field;
@end

@implementation ViewController

- (void)viewDidLoad {
  [super viewDidLoad];
  self.view.backgroundColor = UIColor.systemBackgroundColor;

  // The button fills the top half, so a tap a third of the way down the screen lands on it.
  UIButton *button = [UIButton buttonWithType:UIButtonTypeSystem];
  [button setTitle:@"Say hello" forState:UIControlStateNormal];
  button.titleLabel.font = [UIFont boldSystemFontOfSize:32];
  button.backgroundColor = UIColor.systemBlueColor;
  [button setTitleColor:UIColor.whiteColor forState:UIControlStateNormal];
  button.layer.cornerRadius = 24;
  button.accessibilityIdentifier = @"hello";
  [button addTarget:self action:@selector(hello) forControlEvents:UIControlEventTouchUpInside];
  button.translatesAutoresizingMaskIntoConstraints = NO;

  self.label = [[UILabel alloc] init];
  self.label.text = @"Waiting";
  self.label.font = [UIFont systemFontOfSize:24];
  self.label.textAlignment = NSTextAlignmentCenter;
  self.label.accessibilityIdentifier = @"status";
  self.label.translatesAutoresizingMaskIntoConstraints = NO;

  self.field = [[UITextField alloc] init];
  self.field.placeholder = @"Your name";
  self.field.accessibilityLabel = @"Your name";
  self.field.borderStyle = UITextBorderStyleRoundedRect;
  self.field.font = [UIFont systemFontOfSize:22];
  self.field.autocorrectionType = UITextAutocorrectionTypeNo;
  self.field.autocapitalizationType = UITextAutocapitalizationTypeNone;
  self.field.spellCheckingType = UITextSpellCheckingTypeNo;
  self.field.accessibilityIdentifier = @"name";
  self.field.delegate = self;
  self.field.translatesAutoresizingMaskIntoConstraints = NO;

  [self.view addSubview:button];
  [self.view addSubview:self.label];
  [self.view addSubview:self.field];
  UILayoutGuide *safe = self.view.safeAreaLayoutGuide;
  [NSLayoutConstraint activateConstraints:@[
    [button.topAnchor constraintEqualToAnchor:safe.topAnchor constant:8],
    [button.bottomAnchor constraintEqualToAnchor:self.view.centerYAnchor constant:-8],
    [button.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor constant:24],
    [button.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor constant:-24],
    [self.label.topAnchor constraintEqualToAnchor:self.view.centerYAnchor constant:24],
    [self.label.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor constant:24],
    [self.label.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor constant:-24],
    [self.field.topAnchor constraintEqualToAnchor:self.label.bottomAnchor constant:24],
    [self.field.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor constant:24],
    [self.field.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor constant:-24],
    [self.field.heightAnchor constraintEqualToConstant:52],
  ]];
  NSLog(@"ghost-e2e: ready");
}

- (void)hello {
  self.label.text = @"Hello from Ghost";
  NSLog(@"ghost-e2e: tapped");
  // What an app prints, as a Swift print() does: its standard output.
  puts("ghost-e2e out: tapped");
  fflush(stdout);
}

- (BOOL)textFieldShouldReturn:(UITextField *)field {
  self.label.text = [NSString stringWithFormat:@"Typed %@", field.text];
  NSLog(@"ghost-e2e: typed %lu characters", (unsigned long)field.text.length);
  [field resignFirstResponder];
  return YES;
}

@end

// The window lives in a scene (Info.plist's UIApplicationSceneManifest): an app built with the
// iOS 27 SDK that does not use scenes is not launched at all.
@interface SceneDelegate : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation SceneDelegate

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)options {
  self.window = [[UIWindow alloc] initWithWindowScene:(UIWindowScene *)scene];
  self.window.rootViewController = [[ViewController alloc] init];
  [self.window makeKeyAndVisible];
  UIOpenURLContext *context = options.URLContexts.anyObject;
  if (context) [self opened:context.URL];
}

// The app's own links (ghoste2e://...), while it runs.
- (void)scene:(UIScene *)scene openURLContexts:(NSSet<UIOpenURLContext *> *)contexts {
  UIOpenURLContext *context = contexts.anyObject;
  if (context) [self opened:context.URL];
}

// The label says which link opened the app.
- (void)opened:(NSURL *)url {
  ViewController *controller = (ViewController *)self.window.rootViewController;
  controller.label.text = [NSString stringWithFormat:@"Opened %@", url.absoluteString];
  NSLog(@"ghost-e2e: opened %@", url.absoluteString);
}

@end

@interface AppDelegate : UIResponder <UIApplicationDelegate>
@end

@implementation AppDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)options {
  return YES;
}

@end

int main(int argc, char *argv[]) {
  @autoreleasepool {
    return UIApplicationMain(argc, argv, nil, NSStringFromClass([AppDelegate class]));
  }
}
