#include "llvm/Pass.h"
#include "llvm/ADT/STLExtras.h"
#include "llvm/Analysis/LoopInfo.h"
#include "llvm/Analysis/MemorySSA.h"
#include "llvm/Analysis/MemorySSAUpdater.h"
#include "llvm/Analysis/RegionInfo.h"
#include "llvm/Analysis/ScalarEvolution.h"
#include "llvm/Analysis/ScalarEvolutionExpressions.h"
#include "llvm/Config/llvm-config.h"
#include "llvm/IR/Constants.h"
#include "llvm/IR/DebugInfo.h"
#include "llvm/IR/DerivedTypes.h"
#include "llvm/IR/Dominators.h"
#include "llvm/IR/Function.h"
#include "llvm/IR/GlobalValue.h"
#include "llvm/IR/IRBuilder.h"
#include "llvm/IR/Intrinsics.h"
#include "llvm/IR/Module.h"
#include "llvm/IR/PassInstrumentation.h"
#include "llvm/IR/PassManager.h"
#include "llvm/IR/Verifier.h"
#include "llvm/Passes/PassBuilder.h"
#include "llvm/Passes/PassPlugin.h"
#include "llvm/Support/Alignment.h"
#include "llvm/Support/raw_ostream.h"
#include "llvm/Transforms/Utils/Cloning.h"
#include "llvm/Transforms/Utils/CodeExtractor.h"
#include "llvm/Transforms/Utils/LoopUtils.h"
#include <llvm/IR/DataLayout.h>

using namespace llvm;

namespace {

static void markFunctionNoOptimize(Function *F) {
  F->addFnAttr(Attribute::OptimizeNone);
  F->addFnAttr(Attribute::NoInline);
}

unsigned getVectorBytes(Type *Ty, DataLayout &DL) {
  auto VecTy = cast<VectorType>(Ty);
  if (VecTy->isScalableTy()) {
    auto ElementTy = VecTy->getElementType();
    return 8 * DL.getTypeAllocSize(ElementTy);
  } else {
    return VecTy->getElementCount().getFixedValue();
  }
}

static Function *cloneInstrumentedFunction(Function *Extracted) {
  FunctionType *OrigTy = Extracted->getFunctionType();

  llvm::SmallVector<Type *> Args{OrigTy->param_begin(), OrigTy->param_end()};
  Args.push_back(PointerType::get(Extracted->getContext(), 0));

  auto NewTy = FunctionType::get(OrigTy->getReturnType(), Args, false);

  auto *F = Function::Create(NewTy, Extracted->getLinkage(),
                             Extracted->getName() + ".instrumented",
                             Extracted->getParent());
  ValueToValueMapTy VMap;

  for (const auto &Arg : enumerate(Extracted->args())) {
    VMap[&Arg.value()] = F->getArg(Arg.index());
  }

  SmallVector<ReturnInst *, 4> Returns;
  CloneFunctionInto(F, Extracted, VMap,
                    CloneFunctionChangeType::LocalChangesOnly, Returns);

  stripDebugInfo(*F);
  markFunctionNoOptimize(F);

  return F;
}

struct MiniperfInstr : PassInfoMixin<MiniperfInstr> {
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &FAM) {
    if (F.hasMetadata("miniperf.generated"))
      return PreservedAnalyses::all();

    auto &LoopInfo = FAM.getResult<LoopAnalysis>(F);
    auto &RI = FAM.getResult<RegionInfoAnalysis>(F);
    auto &DT = FAM.getResult<DominatorTreeAnalysis>(F);

    CodeExtractorAnalysisCache CEAC(F);

    IRBuilder<> Builder(F.getContext());

    auto LoopInfoTy = StructType::create(F.getContext(),
                                         {Type::getInt32Ty(F.getContext()),
                                          PointerType::get(F.getContext(), 0),
                                          PointerType::get(F.getContext(), 0)},
                                         "LoopInfo");

    auto LoopStatsTy = StructType::create(F.getContext(),
                                          {
                                              // Trip count
                                              Type::getInt64Ty(F.getContext()),
                                              // Bytes load
                                              Type::getInt64Ty(F.getContext()),
                                              // Bytes store
                                              Type::getInt64Ty(F.getContext()),
                                              // Scalar int ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Scalar float ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Scalar double ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Vector int ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Vector float ops
                                              Type::getInt64Ty(F.getContext()),
                                              // Vector double ops
                                              Type::getInt64Ty(F.getContext()),
                                          },
                                          "LoopStats");

    Function *NotifyBegin =
        F.getParent()->getFunction("mperf_roofline_internal_notify_loop_begin");
    if (!NotifyBegin) {
      auto FuncTy = FunctionType::get(PointerType::get(F.getContext(), 0),
                                      {PointerType::get(F.getContext(), 0)}, 0);
      NotifyBegin = Function::Create(
          FuncTy, llvm::GlobalValue::ExternalLinkage,
          "mperf_roofline_internal_notify_loop_begin", F.getParent());
    }

    Function *NotifyEnd =
        F.getParent()->getFunction("mperf_roofline_internal_notify_loop_end");
    if (!NotifyEnd) {
      auto FuncTy = FunctionType::get(Type::getVoidTy(F.getContext()),
                                      {PointerType::get(F.getContext(), 0)}, 0);
      NotifyEnd = Function::Create(FuncTy, llvm::GlobalValue::ExternalLinkage,
                                   "mperf_roofline_internal_notify_loop_end",
                                   F.getParent());
    }

    Function *NotifyStats =
        F.getParent()->getFunction("mperf_roofline_internal_notify_loop_stats");
    if (!NotifyStats) {
      auto FuncTy = FunctionType::get(Type::getVoidTy(F.getContext()),
                                      {PointerType::get(F.getContext(), 0),
                                       PointerType::get(F.getContext(), 0)},
                                      0);
      NotifyStats = Function::Create(
          FuncTy, llvm::GlobalValue::ExternalLinkage,
          "mperf_roofline_internal_notify_loop_stats", F.getParent());
    }

    Function *IsInstrEnabled = F.getParent()->getFunction(
        "mperf_roofline_internal_is_instrumented_profiling");
    if (!IsInstrEnabled) {
      auto FuncTy = FunctionType::get(Type::getInt32Ty(F.getContext()), {}, 0);
      IsInstrEnabled = Function::Create(
          FuncTy, llvm::GlobalValue::ExternalLinkage,
          "mperf_roofline_internal_is_instrumented_profiling", F.getParent());
    }

    SmallVector<Loop *> TopLevelLoops;
    llvm::copy_if(LoopInfo, std::back_inserter(TopLevelLoops), [](Loop *L) {
      // Only consider outermost loops
      if (L->getParentLoop())
        return false;

      if (!L->getLoopPreheader()) {
        errs() << "Found a loop without a preheader at " << L->getLocStr()
               << ". Skipping.\n";
        return false;
      }

      if (!L->getExitBlock()) {
        errs() << "Found a loop without an exit block at " << L->getLocStr()
               << ". Skipping.\n";
        return false;
      }

      return true;
    });

    for (auto L : TopLevelLoops) {
      Region *R = RI.getRegionFor(L->getHeader());
      SmallVector<BasicBlock *> RegionBlocks(R->block_begin(), R->block_end());
      CodeExtractor CE(RegionBlocks, &DT);

      size_t LineNo = L->getStartLoc().getLine();
      std::string Filename = L->getLocStr().substr(0, L->getLocStr().find(":"));

      Function *Extracted = CE.extractCodeRegion(CEAC);
      if (!Extracted) {
        errs() << "Failed to outline loop at " << L->getLocStr()
               << ". Skipping.\n";
      }

      Extracted->setMetadata(
          "miniperf.generated",
          MDNode::get(F.getContext(), MDString::get(F.getContext(), "true")));

      // LoopInfo.erase(L);

      ValueToValueMapTy VMap;

      Function *Instrumented = cloneInstrumentedFunction(Extracted);

      CallInst *CallSite = cast<CallInst>(*Extracted->user_begin());

      BasicBlock *CallBB = CallSite->getParent();

      SmallVector<Value *> Outs;
      for (auto &I : *CallBB) {
        bool HasExternalUses = false;
        for (auto *User : I.users()) {
          if (auto *UI = dyn_cast<Instruction>(User)) {
            if (UI->getParent() != CallBB) {
              HasExternalUses = true;
              break;
            }
          }
        }

        if (!HasExternalUses)
          continue;

        Outs.push_back(&I);
      }

      ValueToValueMapTy BlockVMap;
      BasicBlock *InstrBB = CloneBasicBlock(CallBB, BlockVMap);

      F.insert(CallBB->getSingleSuccessor()->getIterator(), InstrBB);

      auto DispatchBB = BasicBlock::Create(F.getContext(), "", &F, CallBB);
      auto LandingPadBB = BasicBlock::Create(F.getContext(), "", &F,
                                             CallBB->getSingleSuccessor());

      CallBB->replaceSuccessorsPhiUsesWith(LandingPadBB);
      CallBB->replaceAllUsesWith(DispatchBB);
      InstrBB->replaceSuccessorsPhiUsesWith(LandingPadBB);
      InstrBB->replaceAllUsesWith(DispatchBB);

      Builder.SetInsertPoint(DispatchBB);
      Value *IsEnabled = Builder.CreateCall(IsInstrEnabled);
      Value *Cmp = Builder.CreateCmp(
          CmpInst::ICMP_NE, IsEnabled,
          ConstantInt::get(Type::getInt32Ty(F.getContext()), 0));

      Value *InfoMem = Builder.CreateAlloca(LoopInfoTy);

      Value *FilenameVar = Builder.CreateGlobalString(Filename);
      Value *FuncNameVar = Builder.CreateGlobalString(F.getName());

      Value *LineNoPtr = Builder.CreateConstGEP2_32(LoopInfoTy, InfoMem, 0, 0);
      Value *FilenamePtr =
          Builder.CreateConstGEP2_32(LoopInfoTy, InfoMem, 0, 1);
      Value *FuncNamePtr =
          Builder.CreateConstGEP2_32(LoopInfoTy, InfoMem, 0, 2);

      Builder.CreateStore(FilenameVar, FilenamePtr);
      Builder.CreateStore(FuncNameVar, FuncNamePtr);
      Builder.CreateStore(
          ConstantInt::get(Type::getInt32Ty(F.getContext()), LineNo),
          LineNoPtr);
      Value *LoopHandle = Builder.CreateCall(NotifyBegin, {InfoMem});

      Builder.CreateCondBr(Cmp, InstrBB, CallBB);

      Builder.SetInsertPoint(LandingPadBB);

      for (auto V : Outs) {
        PHINode *PHI = Builder.CreatePHI(V->getType(), 2);
        PHI->addIncoming(V, CallBB);
        PHI->addIncoming(BlockVMap[V], InstrBB);
        V->replaceUsesOutsideBlock(PHI, LandingPadBB);
      }

      Builder.CreateCall(NotifyEnd, {LoopHandle});

      Builder.CreateBr(CallBB->getSingleSuccessor());

      cast<BranchInst>(CallBB->getTerminator())->setSuccessor(0, LandingPadBB);
      cast<BranchInst>(InstrBB->getTerminator())->setSuccessor(0, LandingPadBB);

      for (auto &&I : *InstrBB) {
        if (auto *Call = dyn_cast<CallInst>(&I)) {
          if (Call->getCalledFunction() == Extracted) {
            SmallVector<Value *> Operands{Call->arg_begin(), Call->arg_end()};
            Operands.push_back(LoopHandle);
            Builder.SetInsertPoint(Call);
            Builder.CreateCall(Instrumented, Operands);
            Call->eraseFromParent();
            break;
          }
        }
      }

      DominatorTree InstrDT(*Instrumented);
      class LoopInfo InstrLI(InstrDT);

      // FIXME: for some reason this is not working properly. Need to find a way
      // to actually extract a loop from here.
      if (InstrLI.begin() == InstrLI.end()) {
        continue;
      }

      auto OutermostLoop = *InstrLI.begin();
      assert(OutermostLoop->isOutermost() &&
             "Expected first loop to be outermost");

      Builder.SetInsertPoint(
          Instrumented->getEntryBlock().getFirstInsertionPt());

      // Create necessary data structures
      Value *StatsMem =
          Builder.CreateAlloca(LoopStatsTy, nullptr, "loop_stats");
      Builder.CreateMemSet(StatsMem,
                           ConstantInt::get(Type::getInt8Ty(F.getContext()), 0),
                           8 * 9, Align(8));

      auto UpdateStats = [&](uint64_t Counter, size_t Idx) {
        if (Counter == 0)
          return;

        Value *Ptr =
            Builder.CreateConstInBoundsGEP2_32(LoopStatsTy, StatsMem, 0, Idx);
        Value *Old = Builder.CreateLoad(Type::getInt64Ty(F.getContext()), Ptr);
        Value *New = Builder.CreateAdd(
            Old, ConstantInt::get(Type::getInt64Ty(F.getContext()), Counter));
        Builder.CreateStore(New, Ptr);
      };

      for (auto *BB : OutermostLoop->getBlocks()) {
        uint64_t BytesLoad = 0;
        uint64_t BytesStore = 0;
        uint64_t ScalarIntOps = 0;
        uint64_t ScalarFloatOps = 0;
        uint64_t ScalarDoubleOps = 0;
        uint64_t VectorIntOps = 0;
        uint64_t VectorFloatOps = 0;
        uint64_t VectorDoubleOps = 0;

        auto DL = F.getParent()->getDataLayout();

        for (auto &&I : *BB) {
          switch (I.getOpcode()) {
          case Instruction::Load:
            if (I.getType()->isVectorTy()) {
              BytesLoad += getVectorBytes(I.getType(), DL);
            } else {
              BytesLoad += DL.getTypeAllocSize(I.getType());
            }
            break;
          case Instruction::Store:
            if (I.getOperand(0)->getType()->isVectorTy()) {
              BytesStore += getVectorBytes(I.getOperand(0)->getType(), DL);
            } else {
              BytesStore += DL.getTypeAllocSize(I.getOperand(0)->getType());
            }
            break;
          case Instruction::Add:
          case Instruction::Sub:
          case Instruction::Shl:
          case Instruction::Mul:
          case Instruction::CompareUsingScalarTypes:
            if (I.getType()->isVectorTy()) {
              VectorIntOps += getVectorBytes(I.getType(), DL);
            } else {
              ScalarIntOps += 1;
            }
            break;
          case Instruction::FAdd:
          case Instruction::FMul:
          case Instruction::FSub:
          case Instruction::FDiv:
          case Instruction::FRem:
          case Instruction::FCmp:
            if (I.getType()->isVectorTy()) {
              auto VecTy = cast<VectorType>(I.getType());
              auto ElementTy = VecTy->getElementType();
              unsigned Multiplier = getVectorBytes(I.getType(), DL);
              if (ElementTy->isFloatTy()) {
                VectorFloatOps += Multiplier;
              } else {
                // FIXME this could actually be half or bfloat
                VectorDoubleOps += Multiplier;
              }
            } else if (I.getType()->isFloatTy()) {
              ScalarFloatOps += 1;
            } else {
              ScalarFloatOps += 1;
            }
            break;
          case Instruction::Call: {
            auto &Call = cast<CallInst>(I);
            if (!isa<IntrinsicInst>(Call))
              break;
            switch (Call.getIntrinsicID()) {
            case Intrinsic::fmuladd:
            case Intrinsic::fma:
              if (I.getType()->isVectorTy()) {
                auto VecTy = cast<VectorType>(I.getType());
                auto ElementTy = VecTy->getElementType();
                unsigned Multiplier = getVectorBytes(I.getType(), DL);
                if (ElementTy->isFloatTy()) {
                  VectorFloatOps += 2 * Multiplier;
                } else {
                  // FIXME this could actually be half or bfloat
                  VectorDoubleOps += 2 * Multiplier;
                }
              } else if (I.getType()->isFloatTy()) {
                ScalarFloatOps += 2;
              } else {
                ScalarFloatOps += 2;
              }
              break;
            case Intrinsic::minnum:
            case Intrinsic::minimum:
            case Intrinsic::maxnum:
            case Intrinsic::maximum:
              if (I.getType()->isVectorTy()) {
                auto VecTy = cast<VectorType>(I.getType());
                auto ElementTy = VecTy->getElementType();
                unsigned Multiplier = getVectorBytes(I.getType(), DL);
                if (ElementTy->isFloatTy()) {
                  VectorFloatOps += Multiplier;
                } else {
                  // FIXME this could actually be half or bfloat
                  VectorDoubleOps += Multiplier;
                }
              } else if (I.getType()->isFloatTy()) {
                ScalarFloatOps += 1;
              } else {
                ScalarFloatOps += 1;
              }
              break;
            }
          }
          }
        }
        Builder.SetInsertPoint(BB->getTerminator());
        UpdateStats(BytesLoad, 1);
        UpdateStats(BytesStore, 2);
        UpdateStats(ScalarIntOps, 3);
        UpdateStats(ScalarFloatOps, 4);
        UpdateStats(ScalarDoubleOps, 5);
        UpdateStats(VectorIntOps, 6);
        UpdateStats(VectorFloatOps, 7);
        UpdateStats(VectorDoubleOps, 8);
      };

      // There's always at least one return in the generated function
      // TODO ensure there's exactly one return
      BasicBlock *RetBlock = &*find_if(*Instrumented, [](BasicBlock &BB) {
        return any_of(BB, [](Instruction &I) { return isa<ReturnInst>(I); });
      });

      Value *LocalHandle = Instrumented->getArg(Instrumented->arg_size() - 1);
      Builder.SetInsertPoint(RetBlock->getTerminator());
      Builder.CreateCall(NotifyStats, {LocalHandle, StatsMem});

      if (verifyFunction(*Instrumented, &llvm::errs())) {
        abort();
      }
    }

    return PreservedAnalyses::none();
  }
};

} // namespace

llvm::PassPluginLibraryInfo getMiniperfPluginInfo() {
  return {LLVM_PLUGIN_API_VERSION, "miniperf", LLVM_VERSION_STRING,
          [](PassBuilder &PB) {
            PB.registerOptimizerLastEPCallback([](llvm::ModulePassManager &PM,
                                                  OptimizationLevel Level
#if LLVM_VERSION_MAJOR >= 20
                                                  ,
                                                  ThinOrFullLTOPhase Phase
#endif
                                               ) {
              PM.addPass(createModuleToFunctionPassAdaptor(MiniperfInstr()));
            });
          }};
}

extern "C" LLVM_ATTRIBUTE_WEAK ::llvm::PassPluginLibraryInfo
llvmGetPassPluginInfo() {
  return getMiniperfPluginInfo();
}
